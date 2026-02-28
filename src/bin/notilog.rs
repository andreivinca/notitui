use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

#[path = "../app_config.rs"]
mod app_config;

#[derive(Debug, Clone)]
struct PendingNotify {
    timestamp: String,
    app_name: String,
    summary: String,
    body: String,
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

fn main() {
    let mut args = env::args().skip(1);
    let result = match args.next().as_deref() {
        Some("logger") => handle_logger(args.collect()),
        Some("mark-user") => handle_mark_user(args.collect()),
        Some("tail") => handle_tail(args.collect()),
        Some("export") => handle_export(),
        Some("stats") => handle_stats(),
        Some("query") => handle_query(args.collect()),
        Some("lookup") => handle_lookup(args.collect()),
        Some("prune") => handle_prune(args.collect()),
        _ => {
            print_help();
            Ok(())
        }
    };

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn print_help() {
    println!("notilog - notification logger and reader");
    println!("\nCommands:");
    println!("  logger run                Listen on D-Bus and append notification events");
    println!("  mark-user --event <uid>   Mark close reason as dismissed-by-user");
    println!("  export                    Print merged records as JSON array");
    println!("  tail [--n N]              Show the last N raw log records (default 20)");
    println!("  stats                     Show log path and record count");
    println!("  query --id <id>           Show merged record for one notification id");
    println!("  lookup --ids <a,b,c>      Print JSON map of id to HH:MM");
    println!("  prune --days <days>       Remove records older than N days");
}

fn handle_logger(args: Vec<String>) -> Result<(), String> {
    match args.as_slice() {
        [cmd] if cmd == "run" => run_logger(),
        _ => Err(String::from("usage: notilog logger run")),
    }
}

fn handle_mark_user(args: Vec<String>) -> Result<(), String> {
    let target_event = match args.as_slice() {
        [flag, value] if flag == "--event" => Some(value.clone()),
        [flag, value] if flag == "--id" => {
            let id = value
                .parse::<u32>()
                .map_err(|_| String::from("--id expects an integer"))?;
            None.or_else(|| Some(format!("id:{id}")))
        }
        _ => {
            return Err(String::from(
                "usage: notilog mark-user --event <uid> (or --id <id>)",
            ));
        }
    };

    let path = log_path()?;
    let max_notification_length = max_notification_length();
    let records = read_records(&path)?;
    let merged = aggregate_records(&records);

    let current = if let Some(event_marker) = target_event {
        if let Some(id_text) = event_marker.strip_prefix("id:") {
            let id = id_text
                .parse::<u32>()
                .map_err(|_| String::from("invalid --id value"))?;
            merged
                .iter()
                .find(|record| record.id == id && record.close_reason_code == Some(1))
        } else {
            merged
                .iter()
                .find(|record| record.event_uid.as_deref() == Some(event_marker.as_str()))
        }
    } else {
        None
    };

    let Some(current) = current else {
        return Err(String::from("target notification not found in log"));
    };

    if current.close_reason_code != Some(1) {
        return Err(format!(
            "notification is not auto-dismissed (current reason: {})",
            current.close_reason.as_deref().unwrap_or("unknown")
        ));
    }

    let closed_epoch = now_epoch();
    let closed_hhmm = now_hhmm().unwrap_or_else(|| String::from("--:--"));

    let payload = json!({
        "event_uid": current.event_uid.clone(),
        "id": current.id,
        "close_reason_code": 2,
        "close_reason": "dismissed-by-user",
        "closed_epoch": closed_epoch,
        "closed_hhmm": closed_hhmm,
    });

    append_payload(&path, &payload, max_notification_length)?;

    println!(
        "updated event {} close reason to dismissed-by-user",
        current.event_uid.as_deref().unwrap_or("<unknown-event>")
    );
    Ok(())
}

fn handle_tail(args: Vec<String>) -> Result<(), String> {
    let mut count = 20usize;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--n" {
            let Some(value) = iter.next() else {
                return Err(String::from("usage: notilog tail [--n N]"));
            };
            count = value
                .parse::<usize>()
                .map_err(|_| String::from("--n expects a positive integer"))?;
        } else {
            return Err(String::from("usage: notilog tail [--n N]"));
        }
    }

    let path = log_path()?;
    let records = read_records(&path)?;
    let len = records.len();
    let start = len.saturating_sub(count);

    for record in &records[start..] {
        let id = record.id;
        let hhmm = record
            .hhmm
            .as_deref()
            .or(record.closed_hhmm.as_deref())
            .unwrap_or("--:--");
        let summary = record.summary.as_deref().unwrap_or("(no summary)");
        let suffix = record
            .close_reason
            .as_deref()
            .map(|reason| format!(" [closed:{reason}]"))
            .unwrap_or_default();
        println!("#{id} {hhmm} {summary}{suffix}");
    }

    Ok(())
}

fn handle_export() -> Result<(), String> {
    let path = log_path()?;
    let records = read_records(&path)?;
    let merged = aggregate_records(&records);

    let payload = merged
        .into_iter()
        .map(|record| record_to_json(&record))
        .collect::<Vec<_>>();

    println!(
        "{}",
        serde_json::to_string(&payload)
            .map_err(|error| format!("could not encode export payload: {error}"))?
    );
    Ok(())
}

fn handle_stats() -> Result<(), String> {
    let path = log_path()?;
    let records = read_records(&path)?;
    println!("path: {}", path.display());
    println!("records: {}", records.len());
    Ok(())
}

fn handle_query(args: Vec<String>) -> Result<(), String> {
    let id = parse_single_u32_flag(&args, "--id")?;
    let path = log_path()?;
    let records = read_records(&path)?;
    let merged = aggregate_records(&records);

    let found = merged.into_iter().find(|record| record.id == id);
    if let Some(record) = found {
        println!(
            "{}",
            serde_json::to_string(&record_to_json(&record))
                .map_err(|error| format!("could not encode query result: {error}"))?
        );
    } else {
        println!("null");
    }

    Ok(())
}

fn handle_lookup(args: Vec<String>) -> Result<(), String> {
    let ids_arg = parse_single_string_flag(&args, "--ids")?;
    let wanted_ids: HashSet<u32> = ids_arg
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<u32>()
                .map_err(|_| format!("invalid id '{part}' in --ids"))
        })
        .collect::<Result<HashSet<_>, _>>()?;

    let path = log_path()?;
    let records = read_records(&path)?;
    let merged = aggregate_records(&records);

    let mut out = serde_json::Map::new();
    for record in merged {
        if !wanted_ids.contains(&record.id) {
            continue;
        }
        if let Some(hhmm) = record.hhmm {
            let key = record.id.to_string();
            out.entry(key).or_insert(Value::String(hhmm));
        }
    }

    println!(
        "{}",
        serde_json::to_string(&Value::Object(out))
            .map_err(|error| format!("could not encode lookup result: {error}"))?
    );

    Ok(())
}

fn handle_prune(args: Vec<String>) -> Result<(), String> {
    let days = parse_single_u64_flag(&args, "--days")?;
    let path = log_path()?;
    let mut records = read_records(&path)?;

    let now = now_epoch();
    let cutoff = now.saturating_sub((days as i64).saturating_mul(24 * 60 * 60));

    let before = records.len();
    records.retain(|record| match event_epoch(record) {
        Some(epoch) => epoch >= cutoff,
        None => true,
    });

    write_records(&path, &records)?;
    let removed = before.saturating_sub(records.len());
    println!("removed: {removed}");
    println!("remaining: {}", records.len());
    Ok(())
}

fn run_logger() -> Result<(), String> {
    let path = log_path()?;
    let max_notification_length = max_notification_length();

    let mut child = Command::new("busctl")
        .args(["--user", "monitor", "org.freedesktop.Notifications"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("could not start busctl monitor: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| String::from("failed to capture busctl stdout"))?;
    let reader = BufReader::new(stdout);

    let mut pending: HashMap<u64, PendingNotify> = HashMap::new();
    let mut active_events: HashMap<u32, String> = HashMap::new();
    let mut block: Vec<String> = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|error| format!("error reading monitor output: {error}"))?;

        if line.starts_with('‣') && line.contains("Type=") {
            process_block(
                &block,
                &mut pending,
                &mut active_events,
                &path,
                max_notification_length,
            )?;
            block.clear();
        }

        if !line.trim().is_empty() || !block.is_empty() {
            block.push(line);
        }
    }

    process_block(
        &block,
        &mut pending,
        &mut active_events,
        &path,
        max_notification_length,
    )?;

    let status = child
        .wait()
        .map_err(|error| format!("could not wait for busctl monitor: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("busctl monitor exited with status {status}"))
    }
}

fn process_block(
    block: &[String],
    pending: &mut HashMap<u64, PendingNotify>,
    active_events: &mut HashMap<u32, String>,
    path: &PathBuf,
    max_notification_length: usize,
) -> Result<(), String> {
    if block.is_empty() {
        return Ok(());
    }

    let header = &block[0];
    let msg_type = token_value(header, "Type=");

    if msg_type.as_deref() == Some("method_call") && block_contains(block, "Member=Notify") {
        let cookie = token_value(header, "Cookie=").and_then(|value| value.parse::<u64>().ok());
        let timestamp = quoted_value_after(header, "Timestamp=");
        let strings = extract_strings(block);

        if let (Some(cookie), Some(timestamp)) = (cookie, timestamp) {
            if strings.len() >= 4 {
                let notify = PendingNotify {
                    timestamp,
                    app_name: strings[0].clone(),
                    summary: strings[2].clone(),
                    body: strings[3].clone(),
                };
                pending.insert(cookie, notify);
            }
        }

        return Ok(());
    }

    if msg_type.as_deref() == Some("method_return") {
        let reply_cookie =
            token_value(header, "ReplyCookie=").and_then(|value| value.parse::<u64>().ok());
        let Some(reply_cookie) = reply_cookie else {
            return Ok(());
        };

        let Some(notify) = pending.remove(&reply_cookie) else {
            return Ok(());
        };

        let Some(id) = first_uint32(block) else {
            return Ok(());
        };

        let (epoch, hhmm) = timestamp_to_epoch_and_hhmm(&notify.timestamp).unwrap_or((None, None));
        let event_uid = make_event_uid(id, &notify.timestamp);
        active_events.insert(id, event_uid.clone());

        let payload = json!({
            "event_uid": event_uid,
            "id": id,
            "epoch": epoch,
            "hhmm": hhmm,
            "bus_timestamp": notify.timestamp,
            "app_name": notify.app_name,
            "summary": notify.summary,
            "body": notify.body,
        });

        append_payload(path, &payload, max_notification_length)?;
        return Ok(());
    }

    if msg_type.as_deref() == Some("signal") && block_contains(block, "Member=NotificationClosed") {
        let Some(timestamp) = quoted_value_after(header, "Timestamp=") else {
            return Ok(());
        };

        let values = uint32_values(block);
        if values.len() < 2 {
            return Ok(());
        }

        let id = values[0];
        let reason_code = values[1];
        let reason = close_reason_label(reason_code);
        let (closed_epoch, closed_hhmm) =
            timestamp_to_epoch_and_hhmm(&timestamp).unwrap_or((None, None));
        let event_uid = active_events.remove(&id);

        let payload = json!({
            "event_uid": event_uid,
            "id": id,
            "close_reason_code": reason_code,
            "close_reason": reason,
            "closed_epoch": closed_epoch,
            "closed_hhmm": closed_hhmm,
            "closed_bus_timestamp": timestamp,
        });

        append_payload(path, &payload, max_notification_length)?;
    }

    Ok(())
}

fn append_payload(path: &PathBuf, payload: &Value, max_notification_length: usize) -> Result<(), String> {
    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;

    serde_json::to_writer(&mut log_file, payload)
        .map_err(|error| format!("could not write log JSON: {error}"))?;
    writeln!(log_file).map_err(|error| format!("could not write log newline: {error}"))?;
    log_file
        .flush()
        .map_err(|error| format!("could not flush log file: {error}"))?;

    prune_to_max_notifications(path, max_notification_length)
}

fn prune_to_max_notifications(path: &PathBuf, max_notification_length: usize) -> Result<(), String> {
    if max_notification_length == 0 {
        return Ok(());
    }

    let records = read_records(path)?;
    if records.is_empty() {
        return Ok(());
    }

    let before = records.len();
    let trimmed = trim_records_to_latest_notifications(records, max_notification_length);
    if trimmed.len() == before {
        return Ok(());
    }

    write_records(path, &trimmed)
}

fn trim_records_to_latest_notifications(
    records: Vec<LogRecord>,
    max_notification_length: usize,
) -> Vec<LogRecord> {
    let mut order: HashMap<String, (i64, usize)> = HashMap::new();
    for (index, record) in records.iter().enumerate() {
        let key = record_event_key(record, index);
        let epoch = event_epoch(record).unwrap_or(0);
        order
            .entry(key)
            .and_modify(|best| {
                if epoch > best.0 || (epoch == best.0 && index > best.1) {
                    *best = (epoch, index);
                }
            })
            .or_insert((epoch, index));
    }

    if order.len() <= max_notification_length {
        return records;
    }

    let mut ranked = order.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .0
            .cmp(&left.1.0)
            .then_with(|| right.1.1.cmp(&left.1.1))
    });

    let keep = ranked
        .into_iter()
        .take(max_notification_length)
        .map(|(key, _)| key)
        .collect::<HashSet<_>>();

    records
        .into_iter()
        .enumerate()
        .filter_map(|(index, record)| {
            let key = record_event_key(&record, index);
            if keep.contains(&key) {
                Some(record)
            } else {
                None
            }
        })
        .collect()
}

fn aggregate_records(records: &[LogRecord]) -> Vec<LogRecord> {
    let mut merged: HashMap<String, LogRecord> = HashMap::new();
    let mut order: HashMap<String, (i64, usize)> = HashMap::new();

    for (idx, record) in records.iter().enumerate() {
        let key = record
            .event_uid
            .clone()
            .unwrap_or_else(|| format!("legacy:{}:{idx}", record.id));
        let entry = merged
            .entry(key.clone())
            .or_insert_with(|| LogRecord::empty(record.id));
        if entry.event_uid.is_none() {
            entry.event_uid = Some(key.clone());
        }
        entry.merge_from(record);

        let epoch = event_epoch(record).unwrap_or(0);
        match order.get_mut(&key) {
            Some((best_epoch, best_idx)) => {
                if epoch > *best_epoch || (epoch == *best_epoch && idx > *best_idx) {
                    *best_epoch = epoch;
                    *best_idx = idx;
                }
            }
            None => {
                order.insert(key, (epoch, idx));
            }
        }
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

fn record_to_json(record: &LogRecord) -> Value {
    json!({
        "event_uid": record.event_uid,
        "id": record.id,
        "epoch": record.epoch,
        "hhmm": record.hhmm,
        "app_name": record.app_name,
        "summary": record.summary,
        "body": record.body,
        "close_reason_code": record.close_reason_code,
        "close_reason": record.close_reason,
        "closed_epoch": record.closed_epoch,
        "closed_hhmm": record.closed_hhmm,
    })
}

fn event_epoch(record: &LogRecord) -> Option<i64> {
    record.closed_epoch.or(record.epoch)
}

fn close_reason_label(reason_code: u32) -> &'static str {
    match reason_code {
        1 => "expired",
        2 => "dismissed-by-user",
        3 => "closed-by-call",
        4 => "undefined",
        _ => "unknown",
    }
}

fn make_event_uid(id: u32, bus_timestamp: &str) -> String {
    // Keep event ids stable and shell-safe for CLI roundtrips.
    let normalized = bus_timestamp
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    format!("{id}_{normalized}")
}

fn block_contains(block: &[String], needle: &str) -> bool {
    block.iter().any(|line| line.contains(needle))
}

fn token_value(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let token = tail.split_whitespace().next()?;
    Some(token.trim_end_matches(';').trim_matches('"').to_string())
}

fn quoted_value_after(line: &str, key: &str) -> Option<String> {
    let start = line.find(key)? + key.len();
    let tail = &line[start..];
    let first_quote = tail.find('"')? + 1;
    let rest = &tail[first_quote..];
    let end_quote = rest.find('"')?;
    Some(rest[..end_quote].to_string())
}

fn extract_strings(block: &[String]) -> Vec<String> {
    let mut strings = Vec::new();

    for line in block {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("STRING ") {
            continue;
        }

        let Some(start) = trimmed.find('"') else {
            continue;
        };
        let rest = &trimmed[start + 1..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        strings.push(rest[..end].to_string());
    }

    strings
}

fn first_uint32(block: &[String]) -> Option<u32> {
    uint32_values(block).into_iter().next()
}

fn uint32_values(block: &[String]) -> Vec<u32> {
    let mut values = Vec::new();
    for line in block {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("UINT32 ") {
            continue;
        }

        let raw = trimmed
            .trim_start_matches("UINT32 ")
            .trim_end_matches(';')
            .trim();

        if let Ok(value) = raw.parse::<u32>() {
            values.push(value);
        }
    }
    values
}

fn timestamp_to_epoch_and_hhmm(timestamp: &str) -> Option<(Option<i64>, Option<String>)> {
    let output = Command::new("date")
        .args(["-d", timestamp, "+%s %H:%M"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let epoch = parts.next()?.parse::<i64>().ok();
    let hhmm = parts.next().map(ToString::to_string);
    Some((epoch, hhmm))
}

fn log_path() -> Result<PathBuf, String> {
    let config = app_config::load_or_create();
    let path = config.log_file_path;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    Ok(path)
}

fn max_notification_length() -> usize {
    app_config::load_or_create().max_notification_length
}

fn record_event_key(record: &LogRecord, index: usize) -> String {
    record
        .event_uid
        .clone()
        .unwrap_or_else(|| format!("legacy:{}:{index}", record.id))
}

fn read_records(path: &PathBuf) -> Result<Vec<LogRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file =
        File::open(path).map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let reader = BufReader::new(file);

    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|error| format!("could not read {}: {error}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(record) = value_to_record(&value) {
            records.push(record);
        }
    }

    Ok(records)
}

fn write_records(path: &PathBuf, records: &[LogRecord]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("could not open {} for write: {error}", path.display()))?;

    for record in records {
        let payload = record_to_json(record);

        serde_json::to_writer(&mut file, &payload)
            .map_err(|error| format!("could not encode log record: {error}"))?;
        writeln!(file).map_err(|error| format!("could not write newline: {error}"))?;
    }

    Ok(())
}

fn value_to_record(value: &Value) -> Option<LogRecord> {
    let id = if let Some(id_u64) = value.get("id").and_then(Value::as_u64) {
        u32::try_from(id_u64).ok()?
    } else if let Some(id_str) = value.get("id").and_then(Value::as_str) {
        id_str.parse::<u32>().ok()?
    } else {
        return None;
    };

    let event_uid = opt_non_empty(value.get("event_uid"));
    let epoch = value.get("epoch").and_then(Value::as_i64);
    let hhmm = opt_non_empty(value.get("hhmm"));
    let app_name = opt_non_empty(value.get("app_name"));
    let summary = opt_non_empty(value.get("summary"));
    let body = opt_non_empty(value.get("body"));
    let close_reason_code = value
        .get("close_reason_code")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok());
    let close_reason = opt_non_empty(value.get("close_reason"));
    let closed_epoch = value.get("closed_epoch").and_then(Value::as_i64);
    let closed_hhmm = opt_non_empty(value.get("closed_hhmm"));

    Some(LogRecord {
        event_uid,
        id,
        epoch,
        hhmm,
        app_name,
        summary,
        body,
        close_reason_code,
        close_reason,
        closed_epoch,
        closed_hhmm,
    })
}

fn opt_non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn parse_single_string_flag(args: &[String], flag: &str) -> Result<String, String> {
    match args {
        [found, value] if found == flag => Ok(value.clone()),
        _ => Err(format!("usage: notilog {} <value>", flag)),
    }
}

fn parse_single_u32_flag(args: &[String], flag: &str) -> Result<u32, String> {
    let value = parse_single_string_flag(args, flag)?;
    value
        .parse::<u32>()
        .map_err(|_| format!("{flag} expects an integer"))
}

fn parse_single_u64_flag(args: &[String], flag: &str) -> Result<u64, String> {
    let value = parse_single_string_flag(args, flag)?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag} expects an integer"))
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
