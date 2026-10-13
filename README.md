# ntfy-toast

A small background daemon for Windows that subscribes to [ntfy](https://ntfy.sh) topics
and shows each message as a native Windows toast notification.

- One standalone `.exe` (~2 MB, no runtime or installer needed, no console window)
- Works with ntfy.sh or self-hosted servers, with access tokens or username/password
- Multiple subscriptions and topics, with a per-subscription minimum priority
- Survives network drops and sleep: reconnects with backoff and resumes from the last
  message seen, so nothing is missed
- Optionally catches up on messages published while it wasn't running
- Picks up config edits live, with no restart needed

[![Rust](https://github.com/daterrell/ntfy-toast/actions/workflows/rust.yml/badge.svg)](https://github.com/daterrell/ntfy-toast/actions/workflows/rust.yml)

## Download

Get `ntfy-toast.exe` from the [latest release](https://github.com/daterrell/ntfy-toast/releases/latest)
([direct link](https://github.com/daterrell/ntfy-toast/releases/latest/download/ntfy-toast.exe)).
It isn't code-signed, so SmartScreen may warn on first run: choose **More info → Run anyway**.
Each release includes a `.sha256` file to check against `Get-FileHash ntfy-toast.exe`.

## Quick start

```powershell
ntfy-toast.exe init        # writes %APPDATA%\ntfy-toast\config.toml
notepad $env:APPDATA\ntfy-toast\config.toml   # set your topic(s)
ntfy-toast.exe test        # shows a sample notification
ntfy-toast.exe install     # start at login, and start now
```

Then publish something:

```sh
curl -H "Title: Hello" -H "Tags: tada" -d "It works" https://ntfy.sh/your-topic
```

Put the exe somewhere permanent (e.g. `%LOCALAPPDATA%\Programs\ntfy-toast\`) before
running `install`, because autostart points at the exe's current location.

## Commands

| Command     | What it does |
|-------------|--------------|
| `run`       | Run the daemon (the default with no command). Exits right away if one is already running. |
| `init`      | Write a sample config (`--force` overwrites). |
| `test`      | Show a test toast (`--title`, `--message`, `--priority 1-5`). |
| `install`   | Add an HKCU `Run` entry so it starts at login, then start it. |
| `uninstall` | Remove the `Run` entry and the notification registration. Stop a running copy with `taskkill /im ntfy-toast.exe`. |
| `paths`     | Print the config, log, state, and cache locations. |

Global options are `--config <file>` and `-v/--verbose` for debug logging.

## Configuration

See [`src/config.rs`](src/config.rs) (`SAMPLE`) for the annotated default.

```toml
default_server = "https://ntfy.sh"
app_display_name = "ntfy"      # name shown in Windows notification settings
click_opens_topic = true       # click opens the topic in the browser if there's no Click URL
catch_up_on_start = false      # on startup, show messages missed while not running

[[subscription]]
topic = "my-alerts"

[[subscription]]
server = "https://ntfy.example.com"
topics = [
  "backups",
  { id = "CgbRePrzudWiLmVl", name = "Cameras" },   # shown as "Cameras" on toasts
]
token = "tk_..."               # or username = "..." + password = "..."
min_priority = 3
name = "Home server"           # attribution line on the toast
```

Each topic is either its id (`"backups"`) or a table with a display `name`. The name
replaces the topic id in the toast's title (when the message has no title of its own)
and in the attribution line, which helps when topic ids are random strings. The id is
still what's subscribed to and what clicking a toast opens.

Changes to the config are applied automatically while the daemon runs (it checks every
2 seconds). Only subscriptions whose settings changed are reconnected; ones that were
already running resume from the last message seen, so nothing is missed during a reload.
If an edit has a mistake, a notification shows the error and the previous settings
stay in effect until the file is fixed.

## How ntfy features map to toasts

| ntfy | Toast |
|------|-------|
| Title / message | Title / body (title defaults to the topic's configured name, or its id) |
| Tags | Emoji shortcodes become an emoji prefix on the title; other tags are listed as `Tags: …` |
| Priority 1 (min) | Silent, goes straight to the notification center with no popup |
| Priority 2 (low) | Silent popup |
| Priority 3 (default) | Default sound |
| Priority 4 (high) | Reminder sound, stays on screen longer |
| Priority 5 (urgent) | Same as 4, plus the `urgent` scenario (shown as an important notification on Windows 11; ignored on Windows 10) |
| Click URL | Opens when the toast is clicked |
| `view` actions | Buttons that open the URL (up to 5) |
| `http` / `broadcast` actions | Not supported (they'd need a COM activator); skipped |
| Icon | Downloaded and shown as the toast's app logo |
| Image attachment (up to 3 MB) | Downloaded and shown inline |
| Other attachment | An "Open <file>" button |

## Files

- Config: `%APPDATA%\ntfy-toast\config.toml`
- Log: `%LOCALAPPDATA%\ntfy-toast\ntfy-toast.log` (rotates at 1 MB)
- Last-seen message ids: `%LOCALAPPDATA%\ntfy-toast\state.json`
- Image cache: `%LOCALAPPDATA%\ntfy-toast\cache\` (files older than a day are pruned at startup)
- Registry: `HKCU\Software\Classes\AppUserModelId\ntfy-toast` (toast name and icon) and
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ntfy-toast` (autostart)

Set `NTFY_TOAST_DATA_DIR` to use a different data directory, for example to run a second,
separate copy for testing (each data directory gets its own single-instance lock).

## Troubleshooting

**The log shows `connect failed` timeouts and nothing arrives.** ntfy-toast uses HTTP/2
when the server offers it, which streams fine through most reverse proxies. If your
server is behind nginx (including Nginx Proxy Manager) and only offers HTTP/1.1, nginx
holds back the stream until `proxy_buffering off` is set for ntfy; see
[ntfy's proxy docs](https://docs.ntfy.sh/config/#nginxapache2caddy). The log points this
out when it happens.

## Building

Native on Windows:

```sh
cargo build --release
```

Cross-compiling from Linux/WSL:

```sh
sudo apt install gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
# -> target/x86_64-pc-windows-gnu/release/ntfy-toast.exe
```

`cargo test` runs on any platform. Off Windows, toasts are printed to stdout instead of shown.

## Releasing

Every push and pull request to `master` runs the tests and builds the exe; the build is
attached to the workflow run as the `ntfy-toast-windows-x64` artifact.

To publish a release, do either of these. There's no version number to edit first: the
tag (`v1.2.3`) sets the version the exe is built with.

- **On GitHub:** Releases → *Draft a new release* → choose a new tag like `v0.3.0` →
  *Publish release*. The exe and its checksum are attached a couple of minutes later.
- **From the command line:**
  ```sh
  git tag v0.3.0
  git push origin v0.3.0
  ```
  This creates the release, with install notes and a changelog.

Tags that aren't version numbers (like `Release`) fail with an error instead of publishing.
