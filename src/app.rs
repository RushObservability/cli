use std::collections::HashSet;

use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use tokio::sync::watch;

use crate::model::{Filter, QuerySpec, TailRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
    Filter,
}

#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    Open(String),
}

pub struct App {
    pub spec: QuerySpec,
    pub records: Vec<TailRecord>,
    pub table_state: TableState,
    pub paused: bool,
    pub pending: Vec<TailRecord>,
    pub pending_count: usize,
    pub new_count: usize,
    pub input_mode: InputMode,
    pub input: String,
    pub show_detail: bool,
    pub show_help: bool,
    pub wrap: bool,
    pub error: Option<String>,
    pub last_updated: Option<DateTime<Utc>>,
    pub web_url: String,
    pub buffer_size: usize,
    query_tx: watch::Sender<QuerySpec>,
}

impl App {
    pub fn new(
        spec: QuerySpec,
        web_url: String,
        buffer_size: usize,
        query_tx: watch::Sender<QuerySpec>,
    ) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self {
            spec,
            records: Vec::new(),
            table_state,
            paused: false,
            pending: Vec::new(),
            pending_count: 0,
            new_count: 0,
            input_mode: InputMode::Normal,
            input: String::new(),
            show_detail: false,
            show_help: false,
            wrap: false,
            error: None,
            last_updated: None,
            web_url,
            buffer_size,
            query_tx,
        }
    }

    pub fn selected(&self) -> Option<&TailRecord> {
        self.table_state
            .selected()
            .and_then(|index| self.records.get(index))
    }

    pub fn receive(&mut self, records: Vec<TailRecord>) {
        self.error = None;
        self.last_updated = Some(Utc::now());
        if self.paused {
            let existing = self
                .records
                .iter()
                .map(TailRecord::key)
                .collect::<HashSet<_>>();
            let records = records
                .into_iter()
                .filter(|record| !existing.contains(&record.key()))
                .collect();
            let before = self.pending.len();
            merge_records(&mut self.pending, records, self.buffer_size);
            self.pending_count += self.pending.len().saturating_sub(before);
            return;
        }
        let before = self
            .records
            .iter()
            .map(TailRecord::key)
            .collect::<HashSet<_>>();
        self.new_count = records
            .iter()
            .filter(|record| !before.contains(&record.key()))
            .count();
        merge_records(&mut self.records, records, self.buffer_size);
        self.clamp_selection();
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        if self.input_mode != InputMode::Normal {
            return self.handle_input_key(key);
        }
        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                self.show_help = false;
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char(' ') => {
                self.toggle_pause();
                Action::None
            }
            KeyCode::Tab => {
                self.spec.signal = self.spec.signal.toggled();
                self.records.clear();
                self.pending.clear();
                self.pending_count = 0;
                self.table_state.select(Some(0));
                self.refresh();
                Action::None
            }
            KeyCode::Char('/') => {
                self.input_mode = InputMode::Search;
                self.input = self.spec.search.clone();
                Action::None
            }
            KeyCode::Char('f') => {
                self.input_mode = InputMode::Filter;
                self.input.clear();
                Action::None
            }
            KeyCode::Char('x') => {
                self.spec.filters.pop();
                self.query_changed();
                Action::None
            }
            KeyCode::Char('c') => {
                self.spec.search.clear();
                self.spec.filters.clear();
                self.query_changed();
                Action::None
            }
            KeyCode::Char('r') => {
                self.refresh();
                Action::None
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Action::None
            }
            KeyCode::Char('w') => {
                self.wrap = !self.wrap;
                Action::None
            }
            KeyCode::Enter => {
                self.show_detail = !self.show_detail;
                Action::None
            }
            KeyCode::Esc => {
                self.show_detail = false;
                Action::None
            }
            KeyCode::Char('o') => match self
                .selected()
                .and_then(|record| record.web_url(&self.web_url).ok())
            {
                Some(url) => Action::Open(url.to_string()),
                None => {
                    self.error = Some("selected row has no web context".to_string());
                    Action::None
                }
            },
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_previous();
                Action::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.table_state.select(Some(0));
                Action::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.table_state
                    .select(Some(self.records.len().saturating_sub(1)));
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.input.clear();
            }
            KeyCode::Enter => {
                match self.input_mode {
                    InputMode::Search => {
                        self.spec.search = self.input.trim().to_string();
                        self.error = None;
                        self.query_changed();
                    }
                    InputMode::Filter => match self.input.parse::<Filter>() {
                        Ok(filter) => {
                            self.spec.filters.push(filter);
                            self.error = None;
                            self.query_changed();
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    },
                    InputMode::Normal => {}
                }
                self.input_mode = InputMode::Normal;
                self.input.clear();
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(character) => self.input.push(character),
            _ => {}
        }
        Action::None
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if !self.paused {
            let pending = std::mem::take(&mut self.pending);
            self.new_count = self.pending_count;
            self.pending_count = 0;
            merge_records(&mut self.records, pending, self.buffer_size);
            self.clamp_selection();
        }
    }

    fn refresh(&mut self) {
        self.pending.clear();
        self.pending_count = 0;
        let _ = self.query_tx.send(self.spec.clone());
    }

    fn query_changed(&mut self) {
        self.records.clear();
        self.pending.clear();
        self.pending_count = 0;
        self.new_count = 0;
        self.show_detail = false;
        self.table_state.select(Some(0));
        self.refresh();
    }

    fn select_next(&mut self) {
        if self.records.is_empty() {
            return;
        }
        let next = self
            .table_state
            .selected()
            .unwrap_or(0)
            .saturating_add(1)
            .min(self.records.len() - 1);
        self.table_state.select(Some(next));
    }

    fn select_previous(&mut self) {
        let previous = self.table_state.selected().unwrap_or(0).saturating_sub(1);
        self.table_state.select(Some(previous));
    }

    fn clamp_selection(&mut self) {
        let selected = self.table_state.selected().unwrap_or(0);
        self.table_state
            .select(Some(selected.min(self.records.len().saturating_sub(1))));
    }
}

fn merge_records(target: &mut Vec<TailRecord>, incoming: Vec<TailRecord>, limit: usize) {
    let mut seen = HashSet::with_capacity(target.len() + incoming.len());
    let mut merged = Vec::with_capacity((target.len() + incoming.len()).min(limit));
    for record in incoming.into_iter().chain(target.drain(..)) {
        if seen.insert(record.key()) {
            merged.push(record);
        }
    }
    merged.sort_unstable_by(|left, right| right.timestamp_ns.cmp(&left.timestamp_ns));
    merged.truncate(limit);
    *target = merged;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::model::Signal;

    use super::*;

    fn record(timestamp_ns: i64) -> TailRecord {
        TailRecord {
            signal: Signal::Logs,
            timestamp_ns,
            service: "gateway".into(),
            level: "info".into(),
            summary: format!("row-{timestamp_ns}"),
            trace_id: String::new(),
            span_id: String::new(),
            duration_ns: None,
            http_method: None,
            http_path: None,
            http_status_code: None,
        }
    }

    fn app() -> App {
        let spec = QuerySpec {
            signal: Signal::Logs,
            search: String::new(),
            filters: vec![],
            window: Duration::from_secs(60),
            limit: 100,
        };
        let (tx, _) = watch::channel(spec.clone());
        App::new(spec, "http://localhost:5173".into(), 100, tx)
    }

    #[test]
    fn pause_buffers_updates_until_resume() {
        let mut app = app();
        app.receive(vec![record(1)]);
        app.toggle_pause();
        app.receive(vec![record(2), record(1)]);
        assert_eq!(app.records[0].timestamp_ns, 1);
        assert_eq!(app.pending_count, 1);
        app.toggle_pause();
        assert_eq!(app.records[0].timestamp_ns, 2);
        assert_eq!(app.pending_count, 0);
    }

    #[test]
    fn merging_deduplicates_and_sorts_newest_first() {
        let mut rows = vec![record(1), record(2)];
        merge_records(&mut rows, vec![record(2), record(3)], 10);
        assert_eq!(
            rows.iter().map(|row| row.timestamp_ns).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }
}
