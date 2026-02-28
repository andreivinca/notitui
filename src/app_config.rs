use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_NOTIFICATIONS: usize = 30;
const DEFAULT_LOG_PATH: &str = "~/.local/state/notilog/log.jsonl";

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub log_file_path: PathBuf,
    pub max_notification_length: usize,
}

pub fn load_or_create() -> AppConfig {
    let home = home_dir();
    let config_path = home.join(".config/notitui/config.toml");
    ensure_default_config_file(&config_path);

    let mut log_file_path = expand_path(DEFAULT_LOG_PATH, &home);
    let mut max_notification_length = DEFAULT_MAX_NOTIFICATIONS;

    if let Ok(content) = fs::read_to_string(&config_path) {
        for line in content.lines() {
            let stripped = line.split('#').next().unwrap_or("").trim();
            if stripped.is_empty() {
                continue;
            }

            let Some((key, value)) = stripped.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if value.is_empty() {
                continue;
            }

            match key {
                "log_file_path" => {
                    log_file_path = expand_path(value, &home);
                }
                "max_notification_length" | "max_notifications" => {
                    if let Ok(parsed) = value.parse::<usize>() {
                        if parsed > 0 {
                            max_notification_length = parsed;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    if let Some(parent) = log_file_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    AppConfig {
        log_file_path,
        max_notification_length,
    }
}

fn ensure_default_config_file(path: &Path) {
    if path.exists() {
        return;
    }

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let default = format!(
        "# notitui/notilog config\n# Notification log file path\nlog_file_path = \"{DEFAULT_LOG_PATH}\"\n\n# Maximum number of notifications to keep\nmax_notification_length = {DEFAULT_MAX_NOTIFICATIONS}\n"
    );
    let _ = fs::write(path, default);
}

fn home_dir() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home);
    }
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn expand_path(input: &str, home: &Path) -> PathBuf {
    if input == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return home.join(rest);
    }

    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        home.join(path)
    }
}
