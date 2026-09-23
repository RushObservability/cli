#![allow(dead_code)] // Shared by integration test binaries with different helpers.

use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Child, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub struct Response {
    pub status: u16,
    pub body: String,
    pub hang: bool,
}
impl Response {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            hang: false,
        }
    }
}

pub struct Server {
    pub url: String,
    pub requests: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    task: Option<thread::JoinHandle<()>>,
}

impl Server {
    pub fn start(responses: Vec<Response>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let shutdown = stop.clone();
        let (tx, requests) = mpsc::channel();
        let task = thread::spawn(move || {
            for response in responses {
                let deadline = Instant::now() + Duration::from_secs(8);
                let mut socket = loop {
                    if shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "timed out waiting for CLI request"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                    assert!(bytes.len() < 64 * 1024);
                }
                let headers = String::from_utf8(bytes.clone()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length < 64 * 1024);
                let mut body = vec![0; length];
                socket.read_exact(&mut body).unwrap();
                bytes.extend(body);
                if tx.send(String::from_utf8(bytes).unwrap()).is_err() {
                    return;
                }
                if response.hang {
                    while !shutdown.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(5));
                    }
                    return;
                }
                let wire = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                let _ = socket.write_all(wire.as_bytes()); // Client cancellation is intentional in several tests.
            }
        });
        Self {
            url,
            requests,
            stop,
            task: Some(task),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(task) = self.task.take() {
            let result = task.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

pub struct Process(pub Child);
impl Process {
    pub fn wait(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if self.0.try_wait().unwrap().is_some() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "CLI did not exit within 8 seconds"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn output(mut self) -> Output {
        self.wait();
        let status = self.0.try_wait().unwrap().unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = self.0.stdout.take() {
            pipe.read_to_end(&mut stdout).unwrap();
        }
        if let Some(mut pipe) = self.0.stderr.take() {
            pipe.read_to_end(&mut stderr).unwrap();
        }
        Output {
            status,
            stdout,
            stderr,
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn command(url: &str) -> (Command, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rush"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("RUSH_") {
            cmd.env_remove(key);
        }
    }
    cmd.arg("--config")
        .arg(path)
        .args(["--url", url, "tail", "--poll-interval-ms", "250"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    (cmd, directory)
}
