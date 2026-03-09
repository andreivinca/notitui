# notitui + notilog

Terminal notification history UI (`notitui`) and background logger (`notilog`) for Wayland notifications.

This file only documents setup. Nothing is auto-configured.

## Preview

![notitui preview](assets/notitui-preview.png)

## Binaries

- `notitui`: TUI app (reads configured log file, default `~/.local/state/notilog/log.jsonl`)
- `notilog`: background logger (`logger run`) that writes the JSONL log

## Config file

Both apps use:

- `~/.config/notitui/config.toml`

If missing, it is created automatically with defaults:

```toml
log_file_path = "~/.local/state/notilog/log.jsonl"
max_notification_length = 30
refresh_signal = 8
```

- `log_file_path`: JSONL log location used by both `notilog` and `notitui`
- `max_notification_length`: how many latest notifications `notilog` keeps (older ones are pruned)
- `refresh_signal`: refresh signal channel (`RTMIN+N`) used for external status bars/listeners (default `8`)

## Download release binaries (no build)

If you do not want to compile from source, download prebuilt binaries from:

- `https://github.com/andreivinca/notitui/releases/latest`

Example with `curl` (Linux x86_64):

```bash
curl -fL -o notitui "https://github.com/andreivinca/notitui/releases/download/v0.1.2/notitui-v0.1.2-linux-x86_64"
curl -fL -o notilog "https://github.com/andreivinca/notitui/releases/download/v0.1.2/notilog-v0.1.2-linux-x86_64"
chmod +x notitui notilog
sudo install -Dm755 notitui /usr/local/bin/notitui
sudo install -Dm755 notilog /usr/local/bin/notilog
```

## Build

From project root:

```bash
cargo build --release --bins
```

Release binaries:

- `target/release/notitui`
- `target/release/notilog`

## Install (recommended)

Install to `/usr/local/bin`:

```bash
sudo install -Dm755 target/release/notitui /usr/local/bin/notitui
sudo install -Dm755 target/release/notilog /usr/local/bin/notilog
```

Verify:

```bash
which notitui
which notilog
```

## Run manually

Terminal 1 (logger):

```bash
notilog logger run
```

Terminal 2 (UI):

```bash
notitui
```

## Omarchy keybinding: open `notitui` with `SUPER + N`

Edit:

- `~/.config/hypr/bindings.conf`

Add:

```ini
bind = SUPER, N, exec, xdg-terminal-exec --app-id=TUI.float -e notitui
```

## Start `notilog` at startup

### Option 1: systemd user service (recommended)

Create service file:

```bash
mkdir -p ~/.config/systemd/user
cat > ~/.config/systemd/user/notilog.service <<'EOF'
[Unit]
Description=Notification logger for notitui
After=graphical-session.target

[Service]
Type=simple
ExecStart=notilog logger run
Restart=always
RestartSec=2

[Install]
WantedBy=default.target
EOF
```

Enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now notilog.service
```

Check status/logs:

```bash
systemctl --user status notilog.service
journalctl --user -u notilog.service -f
```

### Option 2: Omarchy/Hyprland autostart (`exec-once`)

In your setup, use:

- `~/.config/hypr/autostart.conf`

Add:

```ini
exec-once = notilog logger run
```

Then restart your session (or reload Hyprland and verify process).

### Option 3: Shell profile fallback

Add to `~/.profile` or `~/.zprofile`:

```bash
pgrep -x notilog >/dev/null || nohup notilog logger run >/tmp/notilog.log 2>&1 &
```

### Option 4: cron `@reboot` fallback

Edit crontab:

```bash
crontab -e
```

Add:

```cron
@reboot notilog logger run >> /tmp/notilog.log 2>&1
```

## Waybar: open `notitui` from icon

Example module in `~/.config/waybar/config.jsonc`:

```jsonc
"custom/notitui": {
  "format": "",
  "tooltip-format": "Notifications",
  "on-click": "setsid uwsm-app -- xdg-terminal-exec --app-id=org.omarchy.terminal --title=Omarchy -e bash -lc 'notitui'"
}
```

Reload Waybar:

```bash
pkill -USR2 waybar
```

## Notes

- `notitui` starts in `missed` mode and toggles with `F`.
- `d` in `notitui` marks selected auto-dismissed notification as user-dismissed in the log.
- If the logger is not running, the UI will only show existing log data.
