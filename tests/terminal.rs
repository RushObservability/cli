#![cfg(unix)]

mod support;

use std::{
    ffi::CStr,
    fs::{File, OpenOptions},
    io::{Read, Write},
    mem::MaybeUninit,
    os::fd::{AsRawFd, FromRawFd},
    process::Stdio,
    thread,
    time::{Duration, Instant},
};
use support::{Process, Response, Server, command};

struct Pty {
    master: File,
    slave: File,
    original: libc::termios,
    control_tail: Vec<u8>,
}

impl Pty {
    fn new() -> Self {
        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 30,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty initializes both owned descriptors on success; the size is valid.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size,
                )
            },
            0
        );
        // SAFETY: These newly allocated descriptors are each transferred to exactly one File.
        let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        // SAFETY: fcntl operates on the live master descriptor.
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let original = Self::settings(&slave);
        Self {
            master,
            slave,
            original,
            control_tail: Vec::new(),
        }
    }

    fn settings(slave: &File) -> libc::termios {
        let mut settings = MaybeUninit::uninit();
        // SAFETY: tcgetattr writes a termios on success, checked before assume_init.
        assert_eq!(
            unsafe { libc::tcgetattr(slave.as_raw_fd(), settings.as_mut_ptr()) },
            0
        );
        unsafe { settings.assume_init() }
    }

    fn read_until(&mut self, marker: &str) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        loop {
            self.drain(&mut output);
            if String::from_utf8_lossy(&output).contains(marker) {
                return output;
            }
            assert!(
                Instant::now() < deadline,
                "terminal did not show {marker}: {}",
                String::from_utf8_lossy(&output)
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn drain(&mut self, output: &mut Vec<u8>) {
        let mut bytes = [0; 8192];
        loop {
            match self.master.read(&mut bytes) {
                Ok(0) => return,
                Ok(count) => {
                    output.extend_from_slice(&bytes[..count]);
                    // A PTY supplies transport, not a terminal emulator. Answer the
                    // cursor-position query issued by ratatui during initialization.
                    self.control_tail.extend_from_slice(&bytes[..count]);
                    for _ in 0..self
                        .control_tail
                        .windows(4)
                        .filter(|bytes| *bytes == b"\x1b[6n")
                        .count()
                    {
                        self.master.write_all(b"\x1b[1;1R").unwrap();
                    }
                    let keep_from = self.control_tail.len().saturating_sub(3);
                    self.control_tail.drain(..keep_from);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => panic!("reading PTY: {error}"),
            }
        }
    }

    fn assert_restored(&self) {
        let now = Self::settings(&self.slave);
        assert_eq!(now.c_iflag, self.original.c_iflag);
        assert_eq!(now.c_oflag, self.original.c_oflag);
        assert_eq!(now.c_cflag, self.original.c_cflag);
        assert_eq!(now.c_lflag, self.original.c_lflag);
        assert_eq!(now.c_cc, self.original.c_cc);
    }

    fn finish(&mut self, mut process: Process) -> (std::process::Output, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut terminal_output = Vec::new();
        // Keep consuming rendered frames while waiting, as a real terminal does.
        // Otherwise the child can block writing a frame into the small PTY buffer.
        while process.0.try_wait().unwrap().is_none() {
            self.drain(&mut terminal_output);
            assert!(Instant::now() < deadline, "CLI did not exit");
            thread::sleep(Duration::from_millis(10));
        }
        self.drain(&mut terminal_output);
        (process.output(), terminal_output)
    }
}

#[test]
fn quit_and_control_c_restore_raw_mode_and_alternate_screen() {
    for key in [b"q".as_slice(), b"\x03".as_slice()] {
        let server = Server::start(vec![Response {
            status: 200,
            body: String::new(),
            hang: true,
        }]);
        let mut pty = Pty::new();
        let (mut cmd, _directory) = command(&server.url);
        cmd.stdin(Stdio::from(pty.slave.try_clone().unwrap()))
            .stdout(Stdio::from(pty.slave.try_clone().unwrap()));
        let process = Process(cmd.spawn().unwrap());
        let mut output = pty.read_until("RUSH");
        assert_eq!(Pty::settings(&pty.slave).c_lflag & libc::ICANON, 0);
        pty.master.write_all(key).unwrap();
        let (result, rest) = pty.finish(process);
        assert!(
            result.status.success(),
            "key {key:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        output.extend(rest);
        assert!(output.windows(8).any(|bytes| bytes == b"\x1b[?1049h"));
        assert!(output.windows(8).any(|bytes| bytes == b"\x1b[?1049l"));
        pty.assert_restored();
    }
}

#[test]
fn api_error_view_can_be_quit_without_leaving_terminal_raw() {
    let server = Server::start(vec![Response::new(500, r#"{"message":"test-outage"}"#)]);
    let mut pty = Pty::new();
    let (mut cmd, _directory) = command(&server.url);
    cmd.stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::from(pty.slave.try_clone().unwrap()));
    let process = Process(cmd.spawn().unwrap());
    pty.read_until("test-outage");
    pty.master.write_all(b"q").unwrap();
    assert!(pty.finish(process).0.status.success());
    pty.assert_restored();
}

#[test]
fn terminal_setup_write_failure_restores_raw_mode() {
    let pty = Pty::new();
    let mut name = [0 as libc::c_char; 1024];
    // SAFETY: ttyname_r writes into the supplied bounded buffer for a live PTY descriptor.
    assert_eq!(
        unsafe { libc::ttyname_r(pty.slave.as_raw_fd(), name.as_mut_ptr(), name.len()) },
        0
    );
    // SAFETY: successful ttyname_r writes a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(name.as_ptr()) }.to_str().unwrap();
    let read_only_stdout = OpenOptions::new().read(true).open(path).unwrap();
    let server = Server::start(vec![]);
    let (mut cmd, _directory) = command(&server.url);
    cmd.stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::from(read_only_stdout));
    let output = Process(cmd.spawn().unwrap()).output();
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    pty.assert_restored();
}
