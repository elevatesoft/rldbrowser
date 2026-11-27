<div align="center">

# RLD Browser

Lightweight, native Rust exam lockdown browser for macOS — with built-in accessibility and zero-retention live proctoring.

[![Latest Release](https://img.shields.io/github/v/release/elevatesoft/rldbrowser?style=flat-square&color=235DBC)](https://github.com/elevatesoft/rldbrowser/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/elevatesoft/rldbrowser/ci.yml?style=flat-square&label=CI)](https://github.com/elevatesoft/rldbrowser/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%2013%2B%20Apple%20Silicon-111111?style=flat-square)
![Binary Size](https://img.shields.io/badge/binary%20size-~3%20MB-2ea44f?style=flat-square)
[![Downloads](https://img.shields.io/github/downloads/elevatesoft/rldbrowser/total?style=flat-square)](https://github.com/elevatesoft/rldbrowser/releases)

[Features](#features) · [Installation](#installation) · [Quick Start](#quick-start) · [Proctor Console](#live-proctor-console) · [Security & Privacy](#security--privacy) · [Building](#building-from-source)

</div>

---

## Overview

**RLD Browser** (Rust LockDown Browser) is a native macOS assessment browser built in Rust on top of `WKWebView` and Apple's `AutomaticAssessmentConfiguration` framework. Instead of bundling Chromium or Electron, it ships as a ~3 MB native binary with fast startup, `.seb` config compatibility, built-in student accessibility controls, and an optional live classroom proctor server (`rld-proctor`).

| | **RLD Browser** | Safe Exam Browser | Respondus LockDown Browser |
| :--- | :--- | :--- | :--- |
| **App Size** | **~3 MB** | ~150–250 MB | ~200–300 MB |
| **Web Engine** | Native `WKWebView` | Chromium / WebKit | Chromium / Electron |
| **`.seb` Config Compatibility** | Yes (`.seb`, `.rld`, `.exam`) | Yes | No |
| **Built-in Accessibility Tools** | Yes (in-session popover) | Limited | No |
| **Live Zero-Retention Proctoring** | Yes (`rld-proctor`, 3 s RAM TTL) | External server | Paid cloud recording |
| **Open Source** | Yes (`MIT OR Apache-2.0`) | Yes (MPL-2.0) | Proprietary |

## Features

### Lockdown & Anti-Bypass
- **Native macOS Assessment Mode**: Engages Apple `AutomaticAssessmentConfiguration` (`AEAssessmentSession`) with automatic fallback to `NSApplicationPresentationOptions` kiosk lockdown.
- **Strict Navigation Gating**: Enforces URL allowlists and blocklists, disables popups, context menus, devtools, downloads, and escape shortcuts (`Cmd+Q`, `Cmd+H`, `Cmd+W`, stripped `NSMenu` items).
- **Environment Hardening**: Clears the pasteboard and stubs `navigator.clipboard`, blocks screen capture, detects mirrored or secondary external displays (`CGGetOnlineDisplayList` / `CGDisplayIsInMirrorSet`), and checks against a screen-capture process denylist.

### Live Proctor Console
- **Self-Hosted Classroom Server**: `rld-proctor` serves a real-time web dashboard over a multiplexed binary WebSocket (`/ws/proctor`) with adaptive FPS scaling (30–60 FPS for the inspected student, throttled background tiles for 30+ student rooms).
- **Camera & Screen PiP**: Streams student webcam feeds alongside zero-permission macOS `WKWebView` screen snapshots—displayed as a bottom-right picture-in-picture overlay on grid tiles and a full 16:9 view with active domain in the Inspector.
- **Live Room Controls**: Multi-room management with 6-character room codes (e.g., `K7M-92P`), live exam policy syncing (allowed tabs, access code, exit password, webcam/clipboard/display/accessibility toggles), and remote session unlock.

### Accessibility Built In
- **In-Session Accessibility Popover**: Students can adjust text scaling, line height, letter spacing, dyslexia-friendly typography, and color tints without leaving the locked exam environment.
- **Tamper-Resistant Injection**: The accessibility bridge (`window.__examA11y`) is frozen on load and can be constrained per exam via policy settings.

### `.seb` & Exam Configs
- **Standard Config Formats**: Opens Safe Exam Browser (`.seb`) files as well as native `.rld` and `.exam` TOML configs.
- **One-Click Launch Links**: Supports `rld://` launch URLs (with `rustbrowser://` alias) and built-in CLI utilities for `config-key`, `seb-key` (`X-SafeExamBrowser-ConfigKeyHash`), `hash-password`, `make-link`, and `check-security`.

## Installation

Download the latest release from [GitHub Releases](https://github.com/elevatesoft/rldbrowser/releases/latest):

- **macOS App**: `RLD-Browser-<version>-macos-arm64.dmg` (drag **RLD Browser.app** into `/Applications`) or `RLD-Browser-<version>-macos-arm64.zip`.
- **Proctor Server (`rld-proctor`)**: Prebuilt standalone binaries for macOS ARM64 (`macos-arm64`), Linux ARM64 (`linux-aarch64`), and Linux x86_64 (`linux-x86_64`).
- **Checksums**: Every release includes `SHA256SUMS.txt` for verifying downloads (`shasum -a 256 -c SHA256SUMS.txt`).

> **Note on ad-hoc signed builds**
> Until Apple Developer ID notarization is enabled, macOS Gatekeeper may warn on first launch. Right-click **RLD Browser.app** → **Open**, or clear the quarantine attribute from Terminal:
>
> ```bash
> xattr -cr "/Applications/RLD Browser.app"
> ```

## Quick Start

1. **Start the proctor server** (prints the dashboard URL and session token on startup):

   ```bash
   ./rld-proctor --room AP-CS-P2
   ```

   Open the printed dashboard URL in a browser to create a teacher account, manage rooms, and inspect connected students.

2. **Launch RLD Browser in windowed dev mode** (safe for local testing; supports `Cmd+Shift+Opt+Q` escape):

   ```bash
   "/Applications/RLD Browser.app/Contents/MacOS/rld-browser" --dev --url https://example.com
   ```

3. **Launch RLD Browser in full kiosk lockdown** (using an exam config or `.seb` file):

   ```bash
   "/Applications/RLD Browser.app/Contents/MacOS/rld-browser" --kiosk --config exam.seb
   ```

## Security & Privacy

- **Zero-Retention Media Pipeline**: All webcam and screen frames live strictly in RAM with a **3-second TTL** and are never written to disk. Frame buffers are explicitly zeroized in memory on replacement, expiration, student exit, and drop.
- **No-Cache Headers & Client Cleanup**: Media and credential endpoints return `Cache-Control: no-store, no-cache, must-revalidate, max-age=0`, and the dashboard revokes `ObjectURL` handles immediately on stale or exited streams.
- **Per-Room Join Tokens**: Enabling `--require-join-token` enforces 128-bit per-room student join tokens via `X-Room-Code` and `X-Room-Token` headers.
- **Multi-Tenant Teacher Isolation**: Rooms are scoped to their owning teacher account; cross-teacher room reads, stream access, and policy mutations return `403 Forbidden`.
- **Native macOS TLS 1.3**: Student-to-proctor `https://` transport uses Apple's native `NSURLSession` stack with TLS 1.3, HTTP/2 connection pooling, and system Keychain root CA trust.
- **Auth Hardening**: Per-IP sliding-window rate limiting (`429 Too Many Requests`) on `/api/auth/login` and `/api/auth/register`, salted password hashes, `HttpOnly` session cookies with automatic `; Secure` flag behind `X-Forwarded-Proto: https`, and a 128-bit secret path token on the local HUD loopback server.

## Building from Source

Requires macOS 13+ (Apple Silicon) and a recent Rust toolchain (`edition = "2024"`).

```bash
# Build release binaries and assemble dist/RLD Browser.app + dist/rld-proctor
./scripts/bundle-macos.sh

# Run lints and workspace tests
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Workspace Layout

| Crate | Description |
| :--- | :--- |
| `crates/core` | Exam config parsing (`.seb`, `.rld`, `.exam`), URL allowlist policy, quit-password hashing, Config Key fingerprinting, and OS-agnostic lockdown/probe traits. |
| `crates/platform` | macOS `AEAssessmentSession` and kiosk lockdown, pasteboard and screen-capture controls, and anti-bypass security probes (with Windows/Linux seams). |
| `crates/app` | Native `WKWebView` exam shell (`rld-browser`), navigation gating, accessibility popover, native `NSURLSession` proctor client, and CLI subcommands. |
| `crates/proctor` | Live classroom proctor server (`rld-proctor`), teacher accounts, multi-room isolation, zero-retention frame channels, and WebSocket dashboard. |

## License

Dual-licensed under either **MIT** or **Apache-2.0** at your option.
