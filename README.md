# RLD Browser

RLD Browser (Rust LockDown Browser) is a lightweight (~3 MB) native Rust exam lockdown browser for macOS with cross-platform seams for future Windows and Linux backends. It supports `.seb`, `.rld`, and `.exam` configurations, built-in accessibility tools, and an optional live LAN proctor console (`rld-proctor`).

## Features

- **Native macOS lockdown**: Uses Apple `AutomaticAssessmentConfiguration` (`AEAssessmentSession`) with automatic fallback to `NSApplicationPresentationOptions` kiosk mode, screen-capture blocking, and pasteboard clearing.
- **Allowlist-only navigation**: Enforces strict URL allowlist/blocklist rules while blocking popups, devtools, context menus, clipboard access, and escape shortcuts.
- **Standard launch formats**: Opens `.seb`, `.rld`, and `.exam` config files as well as `rld://` launch URLs.
- **Accessibility popover**: Built-in controls for text scaling and reading comfort without leaving the locked exam session.
- **Optional live LAN proctor console**: `rld-proctor` provides a tokenized local web dashboard with MJPEG webcam feeds and live screen picture-in-picture monitoring.
- **Zero-Electron footprint**: Pure Rust shell built on native `WKWebView` (`tao` + `wry`), keeping binary size around ~3 MB with fast startup on Apple Silicon.

## Quick Start

Build and assemble `dist/RLD Browser.app` and `dist/rld-proctor`:

```bash
./scripts/bundle-macos.sh
```

Start the optional LAN proctor server and open the tokenized dashboard URL it prints:

```bash
./dist/rld-proctor --room AP-CS-P2
```

Launch the browser in kiosk mode:

```bash
"./dist/RLD Browser.app/Contents/MacOS/rld-browser" --kiosk
```

## Development

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workspace Layout

- `crates/core`: Exam configuration parsing (`.seb`/`.rld`/`.exam`), URL allowlist policy, quit-password hashing, config-key fingerprinting, and OS-agnostic lockdown/probe traits.
- `crates/platform`: macOS `AEAssessmentSession` / kiosk lockdown, pasteboard and screen-capture controls, and anti-bypass security probes (with Windows/Linux stubs).
- `crates/app`: Native `WKWebView` exam shell (`rld-browser`), navigation gating, accessibility popover, and kiosk/dev CLI entry points.
- `crates/proctor`: Optional local LAN proctor server (`rld-proctor`) serving the tokenized instructor dashboard, student status, MJPEG camera streams, and live screen PiP.
