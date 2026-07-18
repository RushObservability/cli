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
use clap::Parser;
use cli::{Cli, Command, OutputMode, TailArgs};
use crossterm::{
    event::{Event, EventStream, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use model::{Filter, QuerySpec, TailRecord};
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
    }
}

async fn run_tail(cli: &Cli, tail: &TailArgs) -> Result<()> {
    let config = config::Config::load(cli, tail)?;
    let filters = tail
        .filters
        .iter()
        .map(|value| value.parse::<Filter>())
        .collect::<Result<Vec<_>>>()?;
    let spec = QuerySpec {
        signal: tail.signal,
        search: tail.search.clone().unwrap_or_default(),
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
                                serde_json::to_writer(&mut output, &record)?;
                                output.write_all(b"\n")?;
                                order.push_back(key);
                                while order.len() > config.buffer_size {
                                    if let Some(expired) = order.pop_front() {
                                        seen.remove(&expired);
                                    }
                                }
                            }
                        }
                        output.flush()?;
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
