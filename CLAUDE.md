# ntfy-toast

Rust daemon for Windows that subscribes to ntfy topics and shows WinRT toasts. See README.md for features and
usage. Hosted on Forgejo as `dave/ntfy-toast` (`origin`); GitHub `daterrell/ntfy-toast` is remote `github`, which
Forgejo push-mirrors to. Default branch is `main`; changes go through PRs that the user merges.

## Build and test (from WSL)

- Tests and lint (what CI runs): `cargo test --locked` and `cargo clippy --locked --all-targets -- -D warnings`.
- Windows exe: `cargo build --release --target x86_64-pc-windows-gnu` (mingw-w64 from apt, rustup in ~/.cargo).
  The Windows-only code (`system.rs`, plus the toast calls) is compiled out or stubbed on Linux, so a green
  `cargo test` doesn't prove it builds; cross-build before pushing changes there.
- Real-Windows test: copy the exe under /mnt/c and run it through WSL interop.

## Layout

- `src/subscriber.rs` streams a topic; `daemon.rs` runs the subscriptions; `message.rs` parses ntfy JSON;
  `toast.rs` shows toasts; `action.rs` turns toast buttons into `ntfy-toast-action:` URIs;
  `system.rs` is the Windows integration (toast identity, URI scheme, autostart, single instance);
  `updater.rs` self-updates; `config.rs` holds the config and its annotated `SAMPLE`.
- `.forgejo/workflows/rust.yml` is CI; `.forgejo/scripts/` publishes and prunes releases.

## Versions and releases

- Never edit the version: `Cargo.toml` says `0.0.0` on purpose. `build.rs` derives it from `git describe`
  (rule in `src/git_version.rs`), so CI and local builds need full history and tags.
- Every merge to `main` publishes a Forgejo-only pre-release "Development build X.Y.Z-dev.N" (newest 2 kept).
  A `v*` tag publishes the formal "ntfy-toast X.Y.Z" on Forgejo and GitHub, with notes from `release-notes.sh`.
- Self-update: `[update]` config has `mode` (notify/auto/off) and `channel` (stable/dev). It checks at start plus
  30 s and when CI posts to https://ntfy.terrell.ltd/ntfy-toast-releases. An install verifies the sha256 and a
  `--version` self-test, renames the running exe to `.old`, then restarts.

## Gotchas

- ntfy.terrell.ltd sits behind nginx that buffers streamed HTTP/1.1, so the client must use HTTP/2 (reqwest does).
- The user runs a live install from C:\Users\Dave\AppData\Local\Programs\ntfy-toast\ntfy-toast.exe, started by an
  HKCU Run key, against ntfy.terrell.ltd. Never overwrite or kill it. Test beside it with `NTFY_TOAST_DATA_DIR`
  and a separate `app_id`; copies with a data dir skip the protocol-handler registration, but still check
  `HKCU\Software\Classes\ntfy-toast-action` after Windows tests.
- `timeout` on the WSL side doesn't kill a Windows process. Kill only the PID you started, with `taskkill.exe /PID`.
- Background-session worktree guards block powershell.exe and commands containing shell variables; run those
  outside the worktree.
