# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-11-25

### Added
- Rebranded to **RLD Browser** (`RLD Browser.app`, `rld://` launch scheme with legacy `rustbrowser://` alias) and **RLD Proctor** (`rld-proctor`).
- Multiplexed binary WebSocket (`/ws/proctor`) with adaptive classroom FPS scaling (30–60 FPS for the focused/inspected student, throttled background grid tiles for 30+ student rooms).
- Teacher accounts with sign-in/registration, salted password hashes, `HttpOnly` session cookies, and multi-room management using 6-character room codes (e.g., `K7M-92P`).
- Live student screen streaming via zero-permission macOS `WKWebView` snapshotting, rendered as a FaceTime-style bottom-right PiP on camera tiles and a full 16:9 live view with active domain in the Inspector.
- Live Exam Settings synced to connected students (access code, exit password, auto-opening allowed website tabs, webcam enforcement, clipboard/display/download/accessibility controls).

### Security
- Added per-session 128-bit proctor auth tokens and teacher session gating across all admin and stream routes.
- Fixed Stored XSS in proctor dashboard HTML escaping, added header/connection/payload DoS bounds, and protected the local HUD loopback server with a 128-bit secret path token.
- Added mirrored external display detection (`CGGetOnlineDisplayList` + `CGDisplayIsInMirrorSet`), expanded the capture process denylist, stripped AppKit `NSMenu` shortcuts in kiosk mode, blocked `Cmd+Q`/`Cmd+H`/`Cmd+W` in the HUD, froze `window.__examA11y`, and stubbed `navigator.clipboard`.

### Fixed
- Fixed macOS `NSView` z-order so the floating Accessibility popover and Quit modal always render above dynamically added tabs.
- Bare allowed domains (e.g., `desmos.com`) now cleanly allow `www.` and subdomain redirects.
- Implemented Chrome-style real-time pointer tab drag reordering and flicker-free tab loading spinners.

## [0.1.0] - 2026-11-24

### Added
- Initial native Rust macOS exam lockdown browser, `.seb` compatibility, Accessibility popover, and local proctor server.
