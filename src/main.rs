mod api;
mod app;
mod cli;
mod config;
mod model;
mod ui;

use std::{
    collections::{HashSet, VecDeque},
    io::{self, IsTerminal, Write},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use api::{ApiError, RushClient};
use app::{Action, App};
use clap::{CommandFactory, Parser};
use cli::{Cli, Command, CompletionsArgs, ManArgs, OutputMode, TailArgs};
use crossterm::{
    event::{Event, EventStream, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use model::{Filter, QuerySpec, TailRecord, parse_search_input};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{mpsc, watch};

#[derive(Debug)]
enum PollEvent {
    Records(Vec<TailRecord>),
    Error(String),
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Tail(tail) => run_tail(&cli, tail).await,
        Command::Completions(args) => {
            write_completions(args, &mut io::stdout());
            Ok(())
        }
        Command::Man(args) => write_man(args, &mut io::stdout()),
    }
}

/// Write a shell completion script for `shell` to `out`.
fn write_completions<W: Write>(args: &CompletionsArgs, out: &mut W) {
    let mut command = Cli::command();
    let name = command.get_name().to_string();
    clap_complete::generate(args.shell, &mut command, name, out);
}

/// Write a roff man page to `out`.
///
/// The top-level page cross-references subcommand pages such as rush-tail(1),
/// so those have to be renderable too or the references dangle.
fn write_man<W: Write>(args: &ManArgs, out: &mut W) -> Result<()> {
    let top = Cli::command();
    let command = match args.command.as_deref() {
        None => top,
        Some(name) => top
            .find_subcommand(name)
            .cloned()
            .map(|sub| {
                // clap wants a 'static name. The process renders one page and
                // exits, so leaking a short string here is inconsequential.
                let page: &'static str = Box::leak(format!("rush-{name}").into_boxed_str());
                sub.name(page)
            })
            .with_context(|| format!("unknown subcommand `{name}`"))?,
    };
    clap_mangen::Man::new(command)
        .render(out)
        .context("failed to render the man page")
}

async fn run_tail(cli: &Cli, tail: &TailArgs) -> Result<()> {
    let config = config::Config::load(cli, tail)?;
    let mut filters = tail
        .filters
        .iter()
        .map(|value| value.parse::<Filter>())
        .collect::<Result<Vec<_>>>()?;
    let (search_filters, search) = tail
        .search
        .as_deref()
        .map(parse_search_input)
        .unwrap_or_default();
    filters.extend(search_filters);
    let spec = QuerySpec {
        signal: tail.signal,
        search,
        filters,
        window: Duration::from_secs(config.window_seconds),
        limit: tail.limit,
    };
    let client = RushClient::new(&config).context("failed to create HTTP client")?;

    match tail.output {
        OutputMode::Json => run_json(client, spec, &config).await,
        OutputMode::Tui => {
            if !io::stdout().is_terminal() {
                bail!("TUI output requires a terminal; use --output json when piping")
            }
            run_tui(client, spec, &config).await
        }
    }
}

/// Serialize one record as a JSON line.
///
/// Serialization goes through a buffer rather than `to_writer` so that every
/// error surfacing from this function is a real `io::Error`, which lets the
/// caller distinguish a closed pipe from a genuine write failure.
fn write_record<W: Write, T: serde::Serialize>(out: &mut W, record: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)
}

/// True when the downstream reader has gone away.
fn is_broken_pipe(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::BrokenPipe
}

async fn run_json(client: RushClient, spec: QuerySpec, config: &config::Config) -> Result<()> {
    let mut seen = HashSet::new();
    let mut order = VecDeque::new();
    let mut interval = tokio::time::interval(Duration::from_millis(config.poll_interval_ms));
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                match client.fetch(&spec).await {
                    Ok(records) => {
                        for record in records.into_iter().rev() {
                            let key = record.key();
                            if seen.insert(key.clone()) {
                                match write_record(&mut output, &record) {
                                    Ok(()) => {}
                                    // `rush tail --output json | head` closes the
                                    // pipe early. For a tail-style tool that is
                                    // normal termination, not a failure.
                                    Err(error) if is_broken_pipe(&error) => return Ok(()),
                                    Err(error) => return Err(error.into()),
                                }
                                order.push_back(key);
                                while order.len() > config.buffer_size {
                                    if let Some(expired) = order.pop_front() {
                                        seen.remove(&expired);
                                    }
                                }
                            }
                        }
                        match output.flush() {
                            Ok(()) => {}
                            Err(error) if is_broken_pipe(&error) => return Ok(()),
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Err(error @ (ApiError::Unauthorized | ApiError::Forbidden)) => return Err(error.into()),
                    Err(error) => eprintln!("rush: {error}"),
                }
            }
        }
    }
    Ok(())
}

async fn run_tui(client: RushClient, spec: QuerySpec, config: &config::Config) -> Result<()> {
    let (query_tx, query_rx) = watch::channel(spec.clone());
    let (poll_tx, mut poll_rx) = mpsc::channel(8);
    let poll_task = tokio::spawn(poll(client, query_rx, poll_tx, config.poll_interval_ms));
    let mut app = App::new(spec, config.web_url.clone(), config.buffer_size, query_tx);

    enable_raw_mode()?;
    let terminal_guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = tui_loop(&mut terminal, &mut app, &mut poll_rx).await;

    poll_task.abort();
    terminal.show_cursor()?;
    drop(terminal);
    drop(terminal_guard);
    result
}

async fn tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    poll_rx: &mut mpsc::Receiver<PollEvent>,
) -> Result<()> {
    let mut events = EventStream::new();
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        match app.handle_key(key) {
                            Action::Quit => return Ok(()),
                            Action::Open(url) => {
                                if let Err(error) = open::that(&url) {
                                    app.fail(format!("failed to open web UI: {error}"));
                                }
                            }
                            Action::None => {}
                        }
                    }
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Err(error)) => app.fail(format!("terminal input failed: {error}")),
                    None => return Ok(()),
                    _ => {}
                }
            }
            update = poll_rx.recv() => {
                match update {
                    Some(PollEvent::Records(records)) => app.receive(records),
                    Some(PollEvent::Error(error)) => app.fail(error),
                    None => return Ok(()),
                }
            }
        }
    }
}

async fn poll(
    client: RushClient,
    mut query_rx: watch::Receiver<QuerySpec>,
    poll_tx: mpsc::Sender<PollEvent>,
    poll_interval_ms: u64,
) {
    loop {
        let spec = query_rx.borrow().clone();
        let update = match client.fetch(&spec).await {
            Ok(records) => PollEvent::Records(records),
            Err(error) => PollEvent::Error(error.to_string()),
        };
        if poll_tx.send(update).await.is_err() {
            return;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(poll_interval_ms)) => {}
            changed = query_rx.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that always reports the downstream reader has closed.
    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
        }
    }

    #[test]
    fn broken_pipe_is_recognised() {
        let err = io::Error::new(io::ErrorKind::BrokenPipe, "closed");
        assert!(is_broken_pipe(&err));
    }

    #[test]
    fn other_io_errors_are_not_broken_pipe() {
        let err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        assert!(!is_broken_pipe(&err));
    }

    #[test]
    fn write_record_surfaces_broken_pipe_as_io_error() {
        let record = serde_json::json!({ "message": "hello" });
        let err = write_record(&mut BrokenPipeWriter, &record)
            .expect_err("a closed pipe must produce an error");
        assert!(
            is_broken_pipe(&err),
            "expected BrokenPipe, got {:?}",
            err.kind()
        );
    }

    #[test]
    fn write_record_emits_one_newline_terminated_json_line() {
        let record = serde_json::json!({ "message": "hello" });
        let mut buf = Vec::new();
        write_record(&mut buf, &record).expect("writing to a Vec cannot fail");
        assert_eq!(buf.iter().filter(|b| **b == b'\n').count(), 1);
        assert!(buf.ends_with(b"\n"));
    }
}
