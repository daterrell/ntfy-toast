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

[![Rust](https://forgejo.terrell.ltd/dave/ntfy-toast/actions/workflows/rust.yml/badge.svg?branch=main)](https://forgejo.terrell.ltd/dave/ntfy-toast/actions?workflow=rust.yml)

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
| `update`    | Install the latest release now (`--check` only reports whether there is one). |

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

## Self-update

ntfy-toast keeps itself up to date. It checks for a new release 30 seconds after it
starts, and again whenever the release topic announces one (CI posts there when it
publishes a release), so running copies hear about updates within seconds. Then it shows
**"ntfy-toast X is available"** with an **Install** button. Installing:

1. downloads the new `ntfy-toast.exe` and checks it against the release's SHA-256,
2. runs the download once (`--version`) to make sure it's the expected version,
3. swaps it in place of the running exe (the old one is kept as `ntfy-toast.exe.old`
   until the next start), and
4. restarts with the same settings, then confirms with an "updated" notification.

If any step fails, the current version keeps running and a notification says why.

```toml
[update]
mode = "notify"     # "notify" (Install button), "auto" (install right away), or "off"
channel = "stable"  # "stable" (formal releases), or "dev" (also every merge to main)
server = "https://forgejo.terrell.ltd"   # a Forgejo/Gitea server, or https://github.com
repo = "dave/ntfy-toast"
token = "..."       # Forgejo token with read access to the repo
github_repo = "daterrell/ntfy-toast"    # used when there's no token; "" to disable
topic = "https://ntfy.terrell.ltd/ntfy-toast-releases"   # "" for startup checks only
```

The defaults point at `forgejo.terrell.ltd`. That server requires sign-in, so add a
token: in Forgejo, **Settings → Applications → Generate new token**, with
**repository: Read** permission. Without a token, ntfy-toast reads the public GitHub
releases instead, which carry the same files. The token is only sent to `server`.

A message on the release topic only prompts a check; what gets installed always comes
from the release server and has to match its checksum.

With `channel = "dev"`, ntfy-toast also offers the development build CI publishes for
every change merged to `main` (see [Releasing](#releasing)), and moves on to the next
formal release when there's one. Development builds are published on Forgejo only, so
the dev channel needs the token; without one it stays on formal releases.
`ntfy-toast update --check` shows which channel is in effect.

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
| `http` actions | Buttons that send the request in the background (no browser window) |
| `broadcast` actions | Android-only; skipped |
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
separate copy for testing (each data directory gets its own single-instance lock). Such a
copy leaves the `ntfy-toast-action:` button handler to the main install.

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

The version comes from the repo's tags (`git describe`, in `build.rs`), never from
Cargo.toml, whose version is a placeholder. A build of tag `v0.6.0` is `0.6.0`; three
commits later it's `0.6.1-dev.3+g1a2b3c4`, which sorts after 0.6.0 and before the next
release. Outside a git checkout it falls back to Cargo.toml's `0.0.0`.

## Releasing

CI (`.forgejo/workflows/rust.yml`) does three things:

- **Pull requests:** run the tests and build the exe. The build is attached to the
  workflow run as the `ntfy-toast-windows-x64` artifact.
- **Every merge to `main`:** also publishes a **development build**, a Forgejo
  pre-release titled like *Development build 0.6.1-dev.3* (tag `v0.6.1-dev.3`), with the
  merged change in its notes. Only the newest 2 are kept; older ones and their tags are
  deleted. Copies with `channel = "dev"` are told about it on the release topic.
- **A version tag:** publishes the **formal release** *ntfy-toast 0.6.0* on Forgejo
  and GitHub, with a *What's new* list of the pull requests merged since the previous
  release, then announces it on the release topic so running copies offer it right away.

There's no version number to edit first: the tag is the version. To publish a formal
release, do either of these:

- **On Forgejo:** Releases → *New release* → choose a new tag like `v0.6.0` on `main` →
  *Publish release*. The exe and its checksum are attached a few minutes later. Notes
  you write there stay on the Forgejo release; GitHub gets the generated ones.
- **From the command line:**
  ```sh
  git tag v0.6.0 origin/main
  git push origin v0.6.0
  ```

A tag like `v1.0.0-rc.1` publishes a formal pre-release: the dev channel offers it,
the stable channel waits for `v1.0.0`. Tags that aren't version numbers (like `Release`)
fail with an error instead of publishing.

The Forgejo releases are made with the job's own token; if that lacks permission, add a
`FORGEJO_RELEASE_TOKEN` repository secret (a token with repository write access).
