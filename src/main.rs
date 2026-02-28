use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Stdout, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::*;
use serde_json::Value;

mod app_config;

const AUTO_REFRESH_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct Notification {
    id: u32,
    event_uid: Option<String>,
    summary: String,
    is_undismissed: bool,
    time_hhmm: Option<String>,
    app_name: Option<String>,
    body: Option<String>,
}

#[derive(Debug, Clone)]
struct LogRecord {
    event_uid: Option<String>,
    id: u32,
    epoch: Option<i64>,
    hhmm: Option<String>,
    app_name: Option<String>,
    summary: Option<String>,
    body: Option<String>,
    close_reason_code: Option<u32>,
    close_reason: Option<String>,
    closed_epoch: Option<i64>,
    closed_hhmm: Option<String>,
}

impl LogRecord {
    fn empty(id: u32) -> Self {
        Self {
            event_uid: None,
            id,
            epoch: None,
            hhmm: None,
            app_name: None,
            summary: None,
            body: None,
            close_reason_code: None,
            close_reason: None,
            closed_epoch: None,
            closed_hhmm: None,
        }
    }

    fn merge_from(&mut self, other: &Self) {
        if other.event_uid.is_some() {
            self.event_uid = other.event_uid.clone();
        }
        if other.epoch.is_some() {
            self.epoch = other.epoch;
        }
        if other.hhmm.is_some() {
            self.hhmm = other.hhmm.clone();
        }
        if other.app_name.is_some() {
            self.app_name = other.app_name.clone();
        }
        if other.summary.is_some() {
            self.summary = other.summary.clone();
        }
        if other.body.is_some() {
            self.body = other.body.clone();
        }
        if other.close_reason_code.is_some() {
            self.close_reason_code = other.close_reason_code;
        }
        if other.close_reason.is_some() {
            self.close_reason = other.close_reason.clone();
        }
        if other.closed_epoch.is_some() {
            self.closed_epoch = other.closed_epoch;
        }
        if other.closed_hhmm.is_some() {
            self.closed_hhmm = other.closed_hhmm.clone();
        }
    }
}

impl Notification {
    fn new(id: u32, summary: String) -> Self {
        Self {
            id,
            event_uid: None,
            summary,
            is_undismissed: false,
            time_hhmm: None,
            app_name: None,
            body: None,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum FilterMode {
    All,
    AutoDismissed,
}

impl FilterMode {
    fn label(self) -> &'static str {
        match self {
            Self::All => "history",
            Self::AutoDismissed => "missed",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::All => Self::AutoDismissed,
            Self::AutoDismissed => Self::All,
        }
    }
}

struct App {
    notifications: Vec<Notification>,
    selected: usize,
    filter: FilterMode,
    status: String,
    should_quit: bool,
    last_refresh: Instant,
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            notifications: Vec::new(),
            selected: 0,
            filter: FilterMode::AutoDismissed,
            status: String::from("Loading notifications..."),
            should_quit: false,
            last_refresh: Instant::now(),
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        match fetch_notifications(self.filter) {
            Ok(notifications) => {
                self.notifications = notifications;
                if self.notifications.is_empty() {
                    self.selected = 0;
                } else {
                    self.selected = self.selected.min(self.notifications.len() - 1);
                }
                self.status = format!(
                    "Loaded {} notifications from {}",
                    self.notifications.len(),
                    self.filter.label()
                );
            }
            Err(error) => {
                self.notifications.clear();
                self.selected = 0;
                self.status = format!("Failed to refresh: {error}");
            }
        }
        self.last_refresh = Instant::now();
    }

    fn toggle_filter(&mut self) {
        self.filter = self.filter.toggle();
        self.refresh();
    }

    fn select_next(&mut self) {
        if self.notifications.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.notifications.len();
    }

    fn select_previous(&mut self) {
        if self.notifications.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.notifications.len() - 1
        } else {
            self.selected - 1
        };
    }

    fn select_first(&mut self) {
        self.selected = 0;
    }

    fn select_last(&mut self) {
        if !self.notifications.is_empty() {
            self.selected = self.notifications.len() - 1;
        }
    }

    fn selected_notification(&self) -> Option<&Notification> {
        self.notifications.get(self.selected)
    }

    fn invoke_selected(&mut self) {
        let Some(_notification) = self.selected_notification() else {
            self.status = String::from("Nothing selected");
            return;
        };
        self.status = String::from("Open action is not available in log-only mode");
    }

    fn mark_selected_as_user_dismissed(&mut self) {
        let Some(notification) = self.selected_notification() else {
            self.status = String::from("Nothing selected");
            return;
        };

        if !notification.is_undismissed {
            self.status = String::from("Selected notification is not auto-dismissed");
            return;
        }

        let Some(event_uid) = notification.event_uid.as_deref() else {
            self.status = String::from("Selected notification has no event id");
            return;
        };

        match mark_notification_user_dismissed(event_uid) {
            Ok(message) => {
                self.status = message;
                self.refresh();
            }
            Err(error) => {
                self.status = format!("Failed to update dismiss reason: {error}");
            }
        }
    }
}

fn main() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new();
    let run_result = run_app(&mut terminal, &mut app);
    let restore_result = restore_terminal(&mut terminal);
    run_result?;
    restore_result
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| render_ui(frame, app))?;

        if app.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                        KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
                        KeyCode::Char('g') => app.select_first(),
                        KeyCode::Char('G') => app.select_last(),
                        KeyCode::Char('f') | KeyCode::Char('F') => app.toggle_filter(),
                        KeyCode::Char('d') => app.mark_selected_as_user_dismissed(),
                        KeyCode::Char('r') => app.refresh(),
                        KeyCode::Enter => app.invoke_selected(),
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    handle_mouse_event(app, mouse, area);
                }
                _ => {}
            }
        } else if app.last_refresh.elapsed() >= AUTO_REFRESH_EVERY {
            app.refresh();
        }
    }
}

fn handle_mouse_event(app: &mut App, mouse: MouseEvent, terminal_area: Rect) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            select_notification_at(app, mouse.column, mouse.row, terminal_area);
        }
        MouseEventKind::ScrollDown => app.select_next(),
        MouseEventKind::ScrollUp => app.select_previous(),
        _ => {}
    }
}

fn select_notification_at(app: &mut App, column: u16, row: u16, terminal_area: Rect) {
    if app.notifications.is_empty() {
        return;
    }

    let list_inner = list_inner_area(terminal_area);
    if list_inner.width == 0 || list_inner.height == 0 {
        return;
    }
    if column < list_inner.x
        || column >= list_inner.x + list_inner.width
        || row < list_inner.y
        || row >= list_inner.y + list_inner.height
    {
        return;
    }

    let mut y = row - list_inner.y;
    for (idx, notification) in app.notifications.iter().enumerate() {
        let item_height = notification_item_height(notification);
        if y < item_height {
            app.selected = idx;
            return;
        }
        y -= item_height;

        if idx + 1 < app.notifications.len() {
            // Spacer row between notifications.
            if y == 0 {
                return;
            }
            y -= 1;
        }
    }
}

fn list_inner_area(terminal_area: Rect) -> Rect {
    let area = terminal_area.inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    Block::bordered()
        .padding(Padding::new(0, 0, 1, 0))
        .inner(chunks[0])
}

fn notification_item_height(notification: &Notification) -> u16 {
    let body_lines = notification
        .body
        .as_deref()
        .map(|body| {
            body.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .count()
        })
        .unwrap_or(0);

    1 + u16::try_from(body_lines).unwrap_or(u16::MAX - 1)
}

fn render_ui(frame: &mut Frame, app: &App) {
    let area = frame.area().inner(Margin {
        horizontal: 1,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let mut items: Vec<ListItem> = Vec::new();
    for (idx, notification) in app.notifications.iter().enumerate() {
        let mut lines = Vec::new();
        let summary_color = if notification.is_undismissed {
            Color::Yellow
        } else {
            Color::Green
        };
        let summary = match notification.time_hhmm.as_deref() {
            Some(time) if !time.is_empty() => format!("{time}  {}", notification.summary),
            _ => notification.summary.clone(),
        };
        lines.push(Line::from(summary).style(Style::new().fg(summary_color)));

        if let Some(body) = &notification.body {
            if !body.is_empty() {
                for body_line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
                    lines.push(
                        Line::from(truncate(body_line, 120)).style(Style::new().fg(Color::White)),
                    );
                }
            }
        }
        items.push(ListItem::new(lines));
        if idx + 1 < app.notifications.len() {
            // Dedicated spacer row so it doesn't get selected/highlighted.
            items.push(ListItem::new(Line::from("")));
        }
    }

    let title = format!(
        " Notifications | mode: {} | count: {} ",
        app.filter.label(),
        app.notifications.len()
    );

    let mut state = ListState::default();
    if !app.notifications.is_empty() {
        state.select(Some(app.selected * 2));
    }

    let list = List::new(items)
        .block(
            Block::bordered()
                .title(title)
                .border_style(Style::new().fg(Color::Green))
                .padding(Padding::new(0, 0, 1, 0)),
        )
        .highlight_style(Style::new().bg(Color::DarkGray))
        .highlight_symbol("  ");
    frame.render_stateful_widget(list, chunks[0], &mut state);

    let legend = Paragraph::new(
        "F Show History/Missed | d Mark User Dismissed | r Refresh | q Quit\nk,Up Up | j,Down Down | g Top | G Bottom | mouse click Select",
    )
    .alignment(Alignment::Center)
    .style(Style::new().fg(Color::Cyan))
    .wrap(Wrap { trim: true });
    frame.render_widget(legend, chunks[1]);
}

fn truncate(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect::<String>() + "..."
}

fn fetch_notifications(filter: FilterMode) -> Result<Vec<Notification>, String> {
    load_notifications_from_jsonl(filter)
}

fn load_notifications_from_jsonl(filter: FilterMode) -> Result<Vec<Notification>, String> {
    let path = notification_log_path().ok_or_else(|| String::from("could not resolve log path"))?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let records = read_log_records(&path)?;
    let merged = aggregate_log_records(&records);
    Ok(notifications_from_log_records(&merged, filter))
}

fn notification_log_path() -> Option<PathBuf> {
    Some(app_config::load_or_create().log_file_path)
}

fn mark_notification_user_dismissed(event_uid: &str) -> Result<String, String> {
    let path = notification_log_path().ok_or_else(|| String::from("could not resolve log path"))?;
    let records = read_log_records(&path)?;
    let merged = aggregate_log_records(&records);

    let Some(current) = merged
        .iter()
        .find(|record| record.event_uid.as_deref() == Some(event_uid))
    else {
        return Err(String::from("target notification not found in log"));
    };

    let is_auto_dismissed =
        current.close_reason_code == Some(1) || current.close_reason.as_deref() == Some("expired");
    if !is_auto_dismissed {
        return Err(String::from(
            "selected notification is not currently auto-dismissed",
        ));
    }

    let payload = serde_json::json!({
        "event_uid": current.event_uid.clone(),
        "id": current.id,
        "close_reason_code": 2,
        "close_reason": "dismissed-by-user",
        "closed_epoch": now_epoch(),
        "closed_hhmm": now_hhmm().unwrap_or_else(|| String::from("--:--")),
    });
    append_log_payload(&path, &payload)?;
    Ok(String::from(
        "Marked selected notification as dismissed-by-user",
    ))
}

fn read_log_records(path: &PathBuf) -> Result<Vec<LogRecord>, String> {
    let file =
        File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(record) = parse_log_record(&value) {
            records.push(record);
        }
    }

    Ok(records)
}

fn parse_log_record(value: &Value) -> Option<LogRecord> {
    let id = json_u32(value.get("id"))?;
    Some(LogRecord {
        event_uid: json_string(value.get("event_uid")),
        id,
        epoch: json_i64(value.get("epoch")),
        hhmm: json_string(value.get("hhmm")),
        app_name: json_string(value.get("app_name")),
        summary: json_string(value.get("summary")),
        body: json_string(value.get("body")),
        close_reason_code: json_u32(value.get("close_reason_code")),
        close_reason: json_string(value.get("close_reason")),
        closed_epoch: json_i64(value.get("closed_epoch")),
        closed_hhmm: json_string(value.get("closed_hhmm")),
    })
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn json_u32(value: Option<&Value>) -> Option<u32> {
    let value = value?;
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).ok();
    }
    value.as_str()?.parse::<u32>().ok()
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    value.as_str()?.parse::<i64>().ok()
}

fn aggregate_log_records(records: &[LogRecord]) -> Vec<LogRecord> {
    let mut merged: HashMap<String, LogRecord> = HashMap::new();
    let mut order: HashMap<String, (i64, usize)> = HashMap::new();

    for (index, record) in records.iter().enumerate() {
        let key = record
            .event_uid
            .clone()
            .unwrap_or_else(|| format!("legacy:{}:{index}", record.id));
        let entry = merged
            .entry(key.clone())
            .or_insert_with(|| LogRecord::empty(record.id));
        if entry.event_uid.is_none() {
            entry.event_uid = Some(key.clone());
        }
        entry.merge_from(record);

        let event_epoch = log_record_epoch(record).unwrap_or(0);
        order
            .entry(key)
            .and_modify(|best| {
                if event_epoch > best.0 || (event_epoch == best.0 && index > best.1) {
                    *best = (event_epoch, index);
                }
            })
            .or_insert((event_epoch, index));
    }

    let mut values: Vec<LogRecord> = merged.into_values().collect();
    values.sort_by(|left, right| {
        let left_key = left.event_uid.clone().unwrap_or_default();
        let right_key = right.event_uid.clone().unwrap_or_default();
        let left_order = order.get(&left_key).copied().unwrap_or((0, 0));
        let right_order = order.get(&right_key).copied().unwrap_or((0, 0));
        right_order
            .0
            .cmp(&left_order.0)
            .then_with(|| right_order.1.cmp(&left_order.1))
    });
    values
}

fn notifications_from_log_records(records: &[LogRecord], filter: FilterMode) -> Vec<Notification> {
    records
        .iter()
        .filter_map(|record| {
            let is_auto_dismissed = record.close_reason_code == Some(1)
                || record.close_reason.as_deref() == Some("expired");
            if matches!(filter, FilterMode::AutoDismissed) && !is_auto_dismissed {
                return None;
            }

            let summary = record
                .summary
                .clone()
                .unwrap_or_else(|| String::from("(no summary)"));
            let mut notification = Notification::new(record.id, summary);
            notification.event_uid = record.event_uid.clone();
            notification.is_undismissed = is_auto_dismissed;
            notification.time_hhmm = record.hhmm.clone().or_else(|| record.closed_hhmm.clone());
            notification.app_name = record.app_name.clone();
            notification.body = record.body.clone();
            Some(notification)
        })
        .collect()
}

fn log_record_epoch(record: &LogRecord) -> Option<i64> {
    record.closed_epoch.or(record.epoch)
}

fn append_log_payload(path: &PathBuf, payload: &Value) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("failed to open {} for append: {error}", path.display()))?;
    serde_json::to_writer(&mut file, payload)
        .map_err(|error| format!("failed to write JSON payload: {error}"))?;
    writeln!(file).map_err(|error| format!("failed to append newline: {error}"))?;
    file.flush()
        .map_err(|error| format!("failed to flush {}: {error}", path.display()))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_hhmm() -> Option<String> {
    let output = Command::new("date").arg("+%H:%M").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
