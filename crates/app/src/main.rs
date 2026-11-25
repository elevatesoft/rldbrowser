mod hardening;
mod hud;
mod proctor_client;
mod tabs;

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use clap::{Parser, Subcommand};
use rustbrowser_core::{
    AccessibilityPolicy, BlockReason, ColorTint, ExamConfig, LockdownGuard,
    NavigationDecision, RoomPolicyOverrides, SecurityPolicy, StudentStatus, TabId, TabManager,
    UrlPolicy, compute_seb_request_hash, encode_config_launch_url, evaluate_probes, hash_password,
    parse_launch_url, parse_seb_file, verify_password,
};
use serde::{Deserialize, Serialize};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Fullscreen, WindowBuilder};
use url::Url;
use wry::{NewWindowResponse, PageLoadEvent, Rect, WebView, WebViewBuilder};

#[cfg(target_os = "macos")]
use tao::platform::macos::WindowExtMacOS;

#[derive(Debug, Parser)]
#[command(
    name = "rld-browser",
    about = "RLD Browser — lightweight native Rust exam lockdown browser"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to an exam TOML configuration file
    #[arg(short, long, conflicts_with = "url")]
    config: Option<PathBuf>,

    /// Launch directly to a single HTTPS exam URL (locks navigation to its host)
    #[arg(short, long, conflicts_with = "config")]
    url: Option<Url>,

    /// Optional positional .exam/.toml file path or rld:// launch URL
    #[arg(value_name = "TARGET", conflicts_with_all = ["config", "url"])]
    target: Option<String>,

    /// Safe development mode: windowed by default, enables Cmd+Shift+Opt+Q escape and http://localhost
    #[arg(long)]
    dev: bool,

    /// Force full OS kiosk lockdown even when --dev is enabled
    #[arg(long)]
    kiosk: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate a salted password hash for quit_password_hash in exam.toml
    HashPassword {
        /// Plaintext teacher quit password to hash
        password: String,
    },
    /// Compute the deterministic canonical SHA-256 Config Key for an exam.toml file
    ConfigKey {
        /// Path to the exam TOML configuration file
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Generate a clickable rld://config?data=... launch link from an exam.toml file
    MakeLink {
        /// Path to the exam TOML configuration file
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Inspect a Safe Exam Browser (.seb) file and print its official SEB Config Key (and optional Request Hash)
    SebKey {
        /// Path to the .seb configuration file
        #[arg(short, long)]
        config: PathBuf,
        /// Optional target URL to also compute X-SafeExamBrowser-ConfigKeyHash
        #[arg(short, long)]
        url: Option<Url>,
    },
    /// Run all Phase 2 runtime anti-bypass security probes on this machine and print a report
    CheckSecurity {
        /// Optional exam TOML configuration file (defaults to strict SecurityPolicy)
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

#[derive(Debug)]
enum UserEvent {
    HudReady,
    HudIpc(HudMessage),
    ExamIpc { tab_id: TabId, msg: ExamMessage },
    PageLoad { tab_id: TabId, is_loading: bool },
    NavigationAllowed { tab_id: TabId, host: String },
    NavigationBlocked { reason: String },
    SecurityViolation { findings: Vec<String> },
    ProctorForceUnlock,
    ProctorPolicyUpdated(RoomPolicyOverrides),
    CaptureScreenTick,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HudMessage {
    SwitchTab { id: usize },
    ReorderTab { from_id: usize, to_id: usize },
    ReorderTabs { ids: Vec<usize> },
    NavBack,
    NavForward,
    NavReload,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ToggleHighContrast,
    ToggleReducedMotion,
    SetColorTint { tint: ColorTint },
    ToggleLineFocus,
    ToggleReadableSpacing,
    ReadAloudToggle,
    ReadAloudStop,
    ReadAloudRate { rate: f64 },
    ReadAloudVoice { voice: Option<String> },
    RequestAccessibilityState,
    OpenA11yPopover,
    CloseA11yPopover,
    RecheckSecurity,
    CancelPreflight,
    WebcamFrame { data_url: String },
    WebcamDenied { reason: String },
    WebcamReady,
    OpenQuitModal,
    CancelQuit,
    QuitNoPassword,
    SubmitQuitPassword { password: String },
    SubmitEntranceCode { code: String },
    CancelEntrance,
    DevEmergencyExit,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExamMessage {
    TitleChanged { title: String },
    VoicesLoaded { voices: Vec<String> },
    ZoomIn,
    ZoomOut,
    ZoomReset,
    DevEmergencyExit,
}

#[derive(Debug, Clone, Serialize)]
struct AccessibilitySnapshot {
    allow_read_aloud: bool,
    read_aloud_rate: f64,
    read_aloud_voice: Option<String>,
    available_voices: Vec<String>,
    high_contrast: bool,
    reduced_motion: bool,
    color_tint: ColorTint,
    line_focus: bool,
    readable_spacing: bool,
}

fn sync_hud_accessibility(hud_webview: &WebView, state: &AccessibilitySnapshot) {
    if let Ok(json) = serde_json::to_string(state) {
        let _ = hud_webview.evaluate_script(&format!(
            "window.__hud && window.__hud.setAccessibility && window.__hud.setAccessibility({json});"
        ));
    }
}

fn ensure_hud_on_top(hud_webview: &WebView) {
    #[cfg(target_os = "macos")]
    {
        use wry::WebViewExtMacOS;

        let view = hud_webview.webview();
        let ptr = (&*view as *const _) as *mut std::ffi::c_void;
        // SAFETY: `view` is a live `Retained<WryWebView>` (`NSView` subclass) on the main thread.
        unsafe {
            rustbrowser_platform::bring_view_to_front(ptr);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = hud_webview;
    }
}

fn send_exit_heartbeat(
    proctor_server_url: Option<&Url>,
    base_config: &ExamConfig,
    config_key: &str,
    student_id: &str,
    telemetry: &Arc<Mutex<proctor_client::TelemetryState>>,
) {
    let snap = if let Ok(mut t) = telemetry.lock() {
        t.status = StudentStatus::Exited;
        t.clone()
    } else {
        return;
    };
    let Some(server_url) = proctor_server_url else {
        return;
    };
    let hb = rustbrowser_core::StudentHeartbeat {
        student_id: student_id.to_string(),
        room_code: base_config.proctor.room_code.clone(),
        join_token: base_config.proctor.join_token.clone(),
        exam_name: base_config.exam_name.clone(),
        config_key: config_key.to_string(),
        status: StudentStatus::Exited,
        active_host: snap.active_host,
        violations: snap.violations,
    };
    let _ = proctor_client::post_heartbeat_with_timeout(
        server_url,
        &hb,
        Duration::from_millis(500),
    );
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::HashPassword { password }) => match hash_password(&password) {
            Ok(encoded) => {
                println!("{encoded}");
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                eprintln!("error: failed to hash password: {err}");
                return ExitCode::FAILURE;
            }
        },
        Some(Command::ConfigKey { config }) => match ExamConfig::from_toml_file(&config) {
            Ok(cfg) => {
                println!("{}", cfg.config_key());
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        },
        Some(Command::MakeLink { config }) => match fs::read_to_string(&config) {
            Ok(raw) => match ExamConfig::from_toml_str(&raw) {
                Ok(_) => {
                    println!("{}", encode_config_launch_url(&raw));
                    return ExitCode::SUCCESS;
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    return ExitCode::FAILURE;
                }
            },
            Err(err) => {
                eprintln!("error: failed to read config file: {err}");
                return ExitCode::FAILURE;
            }
        },
        Some(Command::SebKey { config, url }) => match parse_seb_file(&config) {
            Ok(doc) => {
                println!("seb_config_key: {}", doc.seb_config_key);
                if let Some(target) = url {
                    println!(
                        "request_hash:   {}",
                        compute_seb_request_hash(target.as_str(), &doc.seb_config_key)
                    );
                }
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        },
        Some(Command::CheckSecurity { config }) => {
            let policy = match config {
                Some(path) => match ExamConfig::from_toml_file(&path) {
                    Ok(cfg) => cfg.security,
                    Err(err) => {
                        eprintln!("error: {err}");
                        return ExitCode::FAILURE;
                    }
                },
                None => SecurityPolicy::default(),
            };
            let probes = rustbrowser_platform::default_probes();
            let report = evaluate_probes(&probes, &policy);
            println!("{}", report.format_summary());
            return if report.has_blocking_findings(&policy) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
        None => {}
    }

    let mut dev_mode = cli.dev;
    let mut config = match (&cli.config, &cli.url, &cli.target) {
        (Some(path), None, None) => match load_config_file(path) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        },
        (None, Some(url), None) => match ExamConfig::for_url(url.clone()) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        },
        (None, None, Some(target)) => {
            if target.starts_with("rld://") || target.starts_with("rustbrowser://") {
                match parse_launch_url(target) {
                    Ok(cfg) => cfg,
                    Err(err) => {
                        eprintln!("error: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                match load_config_file(&PathBuf::from(target)) {
                    Ok(cfg) => cfg,
                    Err(err) => {
                        eprintln!("error: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        (None, None, None) => {
            // Double-clicked .app bundle with no CLI arguments: launch built-in test exam session.
            dev_mode = !cli.kiosk;
            built_in_demo_config()
        }
        _ => {
            eprintln!(
                "error: pass --config <exam.toml|exam.seb>, --url <https://...>, or a .exam/.seb/rld:// target"
            );
            return ExitCode::FAILURE;
        }
    };

    // In --dev mode, disable OS kiosk lock unless --kiosk was explicitly requested.
    if dev_mode && !cli.kiosk {
        config.lockdown.kiosk_mode = false;
    }

    run_browser(config, dev_mode)
}

fn built_in_demo_config() -> ExamConfig {
    let mut cfg = ExamConfig::for_url(Url::parse("https://example.com/").unwrap()).unwrap();
    cfg.exam_name = "AP CS Unit 4 Quiz".to_string();
    cfg.allowed_domains = vec!["example.com".to_string(), "*.iana.org".to_string()];
    cfg.pinned_tabs = vec![rustbrowser_core::PinnedTab {
        title: "Reference (IANA)".to_string(),
        url: Url::parse("https://www.iana.org/help/example-domains").unwrap(),
    }];
    cfg.accessibility.allow_read_aloud = true;
    cfg.proctor.server_url = Url::parse("http://127.0.0.1:8443").ok();
    cfg.proctor.room_code = Some("AP-CS-P2".to_string());
    cfg.proctor.heartbeat_interval_secs = 2;
    cfg.proctor.enable_webcam = true;
    cfg
}

fn expand_proctor_allowed_domains(
    domains: &[String],
    base_start_host: Option<&str>,
) -> Vec<String> {
    let mut expanded: Vec<String> = Vec::new();
    let mut push_unique = |entry: String| {
        if !entry.is_empty() && !expanded.iter().any(|existing| existing == &entry) {
            expanded.push(entry);
        }
    };

    if let Some(start_host) = base_start_host {
        let normalized_start = start_host.trim().trim_end_matches('.').to_ascii_lowercase();
        push_unique(normalized_start);
    }

    for raw in domains {
        let mut normalized = raw.trim().to_ascii_lowercase();
        if let Some(rest) = normalized
            .strip_prefix("https://")
            .or_else(|| normalized.strip_prefix("http://"))
        {
            normalized = rest.split('/').next().unwrap_or("").to_string();
        }
        let normalized = normalized.trim_end_matches('.');
        if normalized.is_empty() {
            continue;
        }
        if let Some(base) = normalized.strip_prefix("*.") {
            if !base.is_empty() && !base.contains('*') {
                push_unique(base.to_string());
                push_unique(format!("*.{base}"));
            }
        } else if !normalized.contains('*') {
            push_unique(normalized.to_string());
            push_unique(format!("*.{normalized}"));
        }
    }

    expanded
}

fn domain_pattern_to_url(pattern: &str) -> Option<(String, Url)> {
    let cleaned = pattern
        .trim()
        .to_ascii_lowercase()
        .trim_start_matches("*.")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_string();
    if cleaned.is_empty() || !cleaned.contains('.') || cleaned.contains('*') {
        return None;
    }
    let url = Url::parse(&format!("https://{cleaned}/")).ok()?;
    Some((cleaned, url))
}

fn generate_hud_token() -> String {
    let mut bytes = [0u8; 16];
    let filled = fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !filled {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};

        let mut h1 = RandomState::new().build_hasher();
        h1.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let mut h2 = RandomState::new().build_hasher();
        h2.write_u32(std::process::id());
        bytes[..8].copy_from_slice(&h1.finish().to_le_bytes());
        bytes[8..].copy_from_slice(&h2.finish().to_le_bytes());
    }
    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

fn start_hud_loopback_server(html: String) -> Option<String> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = generate_hud_token();
    let expected_path = format!("/{token}");
    let payload = html.into_bytes();
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
            let _ = s.set_write_timeout(Some(Duration::from_millis(250)));
            let mut req_buf = [0u8; 1024];
            let n = s.read(&mut req_buf).unwrap_or(0);
            let req_line = std::str::from_utf8(&req_buf[..n])
                .ok()
                .and_then(|text| text.lines().next())
                .unwrap_or("");
            let mut parts = req_line.split_whitespace();
            let method = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("");
            if method == "GET" && path == expected_path {
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let _ = s.write_all(header.as_bytes());
                let _ = s.write_all(&payload);
            } else {
                let _ = s.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                );
            }
            let _ = s.flush();
        }
    });
    Some(format!("http://127.0.0.1:{port}/{token}"))
}

fn load_config_file(path: &Path) -> Result<ExamConfig, String> {
    let is_seb = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("seb"));
    if is_seb {
        parse_seb_file(path)
            .map(|doc| doc.config)
            .map_err(|e| e.to_string())
    } else {
        ExamConfig::from_toml_file(path).map_err(|e| e.to_string())
    }
}

fn sync_hud_state(hud_webview: &WebView, tab_manager: &TabManager) {
    let active = tab_manager.active_tab();
    let snapshot = tabs::TabStripSnapshot {
        tabs: tab_manager
            .tabs()
            .iter()
            .map(|t| tabs::TabView {
                id: t.id.0,
                title: t.title.clone(),
                host: t.host.clone(),
                is_loading: t.is_loading,
                pinned: t.pinned,
            })
            .collect(),
        active_id: active.id.0,
        zoom_percent: tab_manager.zoom_percent(),
    };

    if let Ok(json) = serde_json::to_string(&snapshot) {
        let _ =
            hud_webview.evaluate_script(&format!("window.__hud && window.__hud.syncTabs({json});"));
    }
    if let Ok(host_arg) = serde_json::to_string(&active.host) {
        let _ = hud_webview
            .evaluate_script(&format!("window.__hud && window.__hud.setHost({host_arg});"));
    }
    let _ = hud_webview.evaluate_script(&format!(
        "window.__hud && window.__hud.setLoading({});",
        active.is_loading
    ));
    let _ = hud_webview.evaluate_script(&format!(
        "window.__hud && window.__hud.setZoom({});",
        tab_manager.zoom_percent()
    ));
}

#[allow(clippy::too_many_arguments)]
fn spawn_tab_webview(
    window: &tao::window::Window,
    proxy: &tao::event_loop::EventLoopProxy<UserEvent>,
    url_policy: &Arc<RwLock<UrlPolicy>>,
    block_downloads_flag: &Arc<AtomicBool>,
    init_script: &str,
    dev_mode: bool,
    tab_id: TabId,
    initial_url: &str,
    bounds: Rect,
    visible: bool,
    cfg: &ExamConfig,
) -> Result<WebView, wry::Error> {
    let nav_proxy = proxy.clone();
    let nav_policy = Arc::clone(url_policy);
    let new_window_policy = Arc::clone(url_policy);
    let exam_ipc_proxy = proxy.clone();
    let load_proxy = proxy.clone();
    let download_proxy = proxy.clone();
    let block_new_windows = cfg.lockdown.block_new_windows;
    let block_downloads = Arc::clone(block_downloads_flag);

    let mut builder = WebViewBuilder::new()
        .with_bounds(bounds)
        .with_url(initial_url)
        .with_incognito(cfg.browser.ephemeral_session)
        .with_devtools(!cfg.lockdown.block_devtools)
        .with_clipboard(!cfg.lockdown.block_clipboard)
        .with_initialization_script(init_script)
        .with_new_window_req_handler(move |url, _features| {
            if block_new_windows {
                return NewWindowResponse::Deny;
            }
            let allowed = new_window_policy
                .read()
                .map(|p| matches!(p.evaluate_str(&url), NavigationDecision::Allow))
                .unwrap_or(false);
            if allowed {
                NewWindowResponse::Allow
            } else {
                NewWindowResponse::Deny
            }
        })
        .with_download_started_handler(move |url, _path| {
            if block_downloads.load(Ordering::Relaxed) {
                if dev_mode {
                    eprintln!("[rld-browser] blocked file download: {url}");
                }
                let _ = download_proxy.send_event(UserEvent::NavigationBlocked {
                    reason: "file downloads are disabled".to_string(),
                });
                false
            } else {
                true
            }
        })
        .with_on_page_load_handler(move |event, _url| {
            let is_loading = matches!(event, PageLoadEvent::Started);
            let _ = load_proxy.send_event(UserEvent::PageLoad { tab_id, is_loading });
        })
        .with_navigation_handler(move |url_str| {
            let decision = nav_policy
                .read()
                .map(|p| p.evaluate_str(&url_str))
                .unwrap_or_else(|_| {
                    NavigationDecision::Block(BlockReason::ExplicitlyBlocked(
                        "navigation policy unavailable".to_string(),
                    ))
                });
            match decision {
                NavigationDecision::Allow => {
                    if let Ok(parsed) = Url::parse(&url_str)
                        && let Some(host) = parsed.host_str()
                    {
                        let _ = nav_proxy.send_event(UserEvent::NavigationAllowed {
                            tab_id,
                            host: host.to_string(),
                        });
                    }
                    true
                }
                NavigationDecision::Block(reason) => {
                    if dev_mode {
                        eprintln!("[rld-browser] blocked navigation to '{url_str}': {reason}");
                    }
                    let _ = nav_proxy.send_event(UserEvent::NavigationBlocked {
                        reason: reason.to_string(),
                    });
                    false
                }
            }
        })
        .with_ipc_handler(move |req| {
            if let Ok(msg) = serde_json::from_str::<ExamMessage>(req.body()) {
                let _ = exam_ipc_proxy.send_event(UserEvent::ExamIpc { tab_id, msg });
            }
        });

    if let Some(ua) = &cfg.browser.user_agent {
        builder = builder.with_user_agent(ua);
    }

    let wv = builder.build_as_child(window)?;
    let _ = wv.set_visible(visible);
    Ok(wv)
}

fn apply_zoom(
    tab_webviews: &BTreeMap<TabId, WebView>,
    hud_webview: &WebView,
    tab_manager: &TabManager,
) {
    let level = tab_manager.zoom_level();
    for wv in tab_webviews.values() {
        let _ = wv.zoom(level);
    }
    let _ = hud_webview.evaluate_script(&format!(
        "window.__hud && window.__hud.setZoom({});",
        tab_manager.zoom_percent()
    ));
}

fn run_browser(config: ExamConfig, dev_mode: bool) -> ExitCode {
    // 1. Prevent debugger attachment in production lockdown sessions.
    if !dev_mode
        && config.security.deny_debugger_attach
        && let Err(err) = rustbrowser_platform::deny_debugger_attach()
    {
        eprintln!("error: failed to enable anti-debug protection: {err}");
        if config.security.fail_closed {
            return ExitCode::FAILURE;
        }
    }

    // 2. Run pre-flight anti-bypass security probes.
    let probes = rustbrowser_platform::default_probes();
    let security_report = evaluate_probes(&probes, &config.security);
    let mut pending_security_findings: Vec<String> = Vec::new();
    if !security_report.is_clean() {
        if dev_mode {
            eprintln!(
                "[rld-browser] security probe findings (non-blocking in --dev):\n{}",
                security_report.format_summary()
            );
        } else if security_report.has_blocking_findings(&config.security) {
            pending_security_findings = security_report
                .critical_findings()
                .map(|f| format!("[{}] {}", f.probe, f.message))
                .collect();
            eprintln!(
                "[rld-browser] pre-flight security gate engaged:\n{}",
                security_report.format_summary()
            );
        } else {
            eprintln!(
                "[rld-browser] security probe warnings:\n{}",
                security_report.format_summary()
            );
        }
    }
    let mut security_blocked = !pending_security_findings.is_empty();

    let url_policy = Arc::new(RwLock::new(UrlPolicy::from_config(&config, dev_mode)));
    let block_downloads_flag = Arc::new(AtomicBool::new(config.browser.block_downloads));
    let mut tab_manager = TabManager::from_config(&config);
    let has_tabs = tab_manager.has_multiple_tabs();
    let mut hud_bar_height = if has_tabs {
        hud::HUD_WITH_TABS_HEIGHT
    } else {
        hud::HUD_BAR_HEIGHT
    };

    let backend = rustbrowser_platform::select_backend_with_accommodations(
        config.security.prefer_assessment_mode,
        config.accessibility.allow_dictation,
        config.accessibility.allow_spell_check,
    );
    let kiosk_active = config.lockdown.kiosk_mode;

    if dev_mode {
        let key = config.config_key();
        eprintln!(
            "[rld-browser] starting '{}' (backend={}, kiosk={}, tabs={}, ephemeral={}, config_key={}..., dev=true)",
            config.exam_name,
            backend.name(),
            kiosk_active,
            tab_manager.tabs().len(),
            config.browser.ephemeral_session,
            &key[..12]
        );
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let mut window_builder = WindowBuilder::new()
        .with_title(&config.exam_name)
        .with_inner_size(LogicalSize::new(1280.0, 820.0));

    if kiosk_active {
        window_builder = window_builder
            .with_decorations(false)
            .with_always_on_top(true)
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
    }

    let window = match window_builder.build(&event_loop) {
        Ok(w) => w,
        Err(err) => {
            eprintln!("error: failed to create window: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Engage OS-level lockdown (RAII guard restores macOS state automatically on exit/panic).
    let mut lockdown_guard: Option<Box<dyn LockdownGuard>> = match backend.enter(&config.lockdown) {
        Ok(guard) => Some(guard),
        Err(err) => {
            if kiosk_active {
                eprintln!("error: failed to enter OS lockdown: {err}");
                return ExitCode::FAILURE;
            }
            if dev_mode {
                eprintln!("[rld-browser] lockdown backend notice: {err}");
            }
            None
        }
    };

    #[cfg(target_os = "macos")]
    {
        let ns_window = window.ns_window();
        if let Err(err) = unsafe { backend.configure_window(ns_window, &config.lockdown) }
            && dev_mode
        {
            eprintln!("[rld-browser] window hardening notice: {err}");
        }
    }

    let mut entrance_unlocked = !config.requires_entrance_code();
    let mut exam_loaded = !security_blocked && entrance_unlocked;
    let initial_size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let initial_hud_height = if exam_loaded {
        hud_bar_height
    } else {
        initial_size.height
    };
    let hud_rect = Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(initial_size.width, initial_hud_height).into(),
    };
    let exam_rect = Rect {
        position: LogicalPosition::new(0.0, hud_bar_height).into(),
        size: LogicalSize::new(
            initial_size.width,
            (initial_size.height - hud_bar_height).max(1.0),
        )
        .into(),
    };

    let hardening_opts = hardening::HardeningOptions {
        block_new_windows: config.lockdown.block_new_windows,
        block_clipboard: config.lockdown.block_clipboard,
        block_context_menu: config.lockdown.block_context_menu,
        block_devtools: config.lockdown.block_devtools,
        allow_reload: config.lockdown.allow_reload,
        allow_zoom: config.browser.allow_zoom,
        allow_find_in_page: config.browser.allow_find_in_page,
        allow_read_aloud: config.accessibility.allow_read_aloud,
        high_contrast: config.accessibility.high_contrast,
        reduced_motion: config.accessibility.reduced_motion,
        color_tint: config.accessibility.color_tint,
        line_focus: config.accessibility.line_focus,
        readable_spacing: config.accessibility.readable_spacing,
        dev_mode,
    };
    let init_script = hardening::build_init_script_with_options(&hardening_opts);

    // Create exam tab WebViews FIRST so `hud_webview` sits above them in native z-order
    // (enabling transparent floating Accessibility Popover and blurred Quit modal over live tabs).
    let mut tab_webviews: BTreeMap<TabId, WebView> = BTreeMap::new();
    for tab in tab_manager.tabs() {
        let initial_tab_url = if exam_loaded {
            tab.url.as_str()
        } else {
            "about:blank"
        };
        let visible = exam_loaded && tab.id == tab_manager.active_id();
        let wv = match spawn_tab_webview(
            &window,
            &proxy,
            &url_policy,
            &block_downloads_flag,
            &init_script,
            dev_mode,
            tab.id,
            initial_tab_url,
            exam_rect,
            visible,
            &config,
        ) {
            Ok(wv) => wv,
            Err(err) => {
                eprintln!("error: failed to create tab {} webview: {err}", tab.id.0);
                return ExitCode::FAILURE;
            }
        };
        tab_webviews.insert(tab.id, wv);
    }

    let hud_proxy = proxy.clone();
    let hud_load_proxy = proxy.clone();
    let hud_html = hud::build_hud_html_with_options(
        &config,
        dev_mode,
        kiosk_active,
        has_tabs,
        config.browser.allow_zoom,
    );
    let hud_builder = WebViewBuilder::new()
        .with_bounds(hud_rect)
        .with_transparent(true)
        .with_background_color((0, 0, 0, 0))
        .with_incognito(true)
        .with_devtools(false)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_on_page_load_handler(move |event, _url| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = hud_load_proxy.send_event(UserEvent::HudReady);
            }
        })
        .with_ipc_handler(move |req| {
            if let Ok(msg) = serde_json::from_str::<HudMessage>(req.body()) {
                let _ = hud_proxy.send_event(UserEvent::HudIpc(msg));
            }
        });
    // Serve HUD over 127.0.0.1 so WKWebView treats it as a Secure Context (enabling getUserMedia).
    let hud_builder = if let Some(hud_url) = start_hud_loopback_server(hud_html.clone()) {
        hud_builder.with_url(&hud_url)
    } else {
        hud_builder.with_html(hud_html)
    };
    let hud_webview = match hud_builder.build_as_child(&window) {
        Ok(wv) => wv,
        Err(err) => {
            eprintln!("error: failed to create HUD webview: {err}");
            return ExitCode::FAILURE;
        }
    };

    // 3. Continuous in-exam background security monitor (non-dev mode).
    let security_policy = Arc::new(Mutex::new(config.security.clone()));
    let monitor_stop = Arc::new(AtomicBool::new(false));
    if !dev_mode && config.security.monitor_interval_secs > 0 {
        let stop_flag = Arc::clone(&monitor_stop);
        let monitor_proxy = proxy.clone();
        let monitor_policy = Arc::clone(&security_policy);
        let interval = Duration::from_secs(config.security.monitor_interval_secs);
        std::thread::spawn(move || {
            let bg_probes = rustbrowser_platform::default_probes();
            while !stop_flag.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                let policy_snap = monitor_policy.lock().unwrap().clone();
                let report = evaluate_probes(&bg_probes, &policy_snap);
                if report.has_blocking_findings(&policy_snap) {
                    let findings: Vec<String> = report
                        .critical_findings()
                        .map(|f| format!("[{}] {}", f.probe, f.message))
                        .collect();
                    if monitor_proxy
                        .send_event(UserEvent::SecurityViolation { findings })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
    }

    // 4. Optional live proctor telemetry client (only active when proctor.server_url is configured).
    let initial_status = if security_blocked {
        StudentStatus::PreflightBlocked
    } else if !entrance_unlocked {
        StudentStatus::WaitingForCode
    } else {
        StudentStatus::InExam
    };
    let telemetry = Arc::new(Mutex::new(proctor_client::TelemetryState {
        status: initial_status,
        active_host: config.start_url.host_str().unwrap_or("").to_string(),
        violations: pending_security_findings.clone(),
    }));
    let student_id = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "student".to_string());
    let proctor_server_url = config.proctor.server_url.clone();
    // Single persistent webcam uploader thread with bounded channel(1) so high-FPS frames never pile up.
    let webcam_tx: Option<mpsc::SyncSender<String>> = proctor_server_url.clone().map(|url| {
        let (tx, rx) = mpsc::sync_channel::<String>(1);
        let sid = student_id.clone();
        let room_code = config.proctor.room_code.clone();
        let join_token = config.proctor.join_token.clone();
        std::thread::spawn(move || {
            let mut uploader =
                proctor_client::WebcamUploader::new(url, sid).with_room(room_code, join_token);
            while let Ok(data_url) = rx.recv() {
                uploader.send_frame(&data_url);
            }
        });
        tx
    });
    // Single persistent screen uploader thread + 8 FPS capture ticker when connected to a proctor server.
    let screen_tx: Option<mpsc::SyncSender<Vec<u8>>> = proctor_server_url.clone().map(|url| {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(1);
        let sid = student_id.clone();
        let room_code = config.proctor.room_code.clone();
        let join_token = config.proctor.join_token.clone();
        std::thread::spawn(move || {
            let mut uploader =
                proctor_client::WebcamUploader::with_endpoint(url, sid, "/api/screen-raw/")
                    .with_room(room_code, join_token);
            while let Ok(jpeg) = rx.recv() {
                uploader.send_raw_jpeg(&jpeg);
            }
        });
        let stop_flag = Arc::clone(&monitor_stop);
        let screen_proxy = proxy.clone();
        std::thread::spawn(move || {
            let interval = Duration::from_millis(125);
            while !stop_flag.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                if screen_proxy
                    .send_event(UserEvent::CaptureScreenTick)
                    .is_err()
                {
                    break;
                }
            }
        });
        tx
    });
    let screen_capture_in_flight = Arc::new(AtomicBool::new(false));
    let config_key = config.config_key();
    if let Some(server_url) = proctor_server_url.clone() {
        let unlock_proxy = proxy.clone();
        let policy_proxy = proxy.clone();
        proctor_client::spawn_heartbeat_thread(
            proctor_client::HeartbeatConfig {
                server_url,
                room_code: config.proctor.room_code.clone(),
                join_token: config.proctor.join_token.clone(),
                student_id: student_id.clone(),
                exam_name: config.exam_name.clone(),
                config_key: config_key.clone(),
                interval_secs: config.proctor.heartbeat_interval_secs,
            },
            Arc::clone(&telemetry),
            Arc::clone(&monitor_stop),
            move || {
                let _ = unlock_proxy.send_event(UserEvent::ProctorForceUnlock);
            },
            move |policy| {
                let _ = policy_proxy.send_event(UserEvent::ProctorPolicyUpdated(policy));
            },
        );
    }

    let base_config = config.clone();
    let mut dynamic_domain_tabs: BTreeMap<String, TabId> = BTreeMap::new();
    let mut webcam_denied_reason: Option<String> = None;
    let mut require_webcam = config.proctor.enable_webcam && config.proctor.server_url.is_some();
    let mut a11y_popover_open = false;
    let mut quit_modal_open = false;
    let mut exit_authorized = false;
    let mut entrance_code_hash = config.entrance_code_hash.clone();
    let mut unlocked_entrance_hash: Option<String> = None;
    let mut quit_password_hash = config.quit_password_hash.clone();
    let block_clipboard = config.lockdown.block_clipboard;
    let mut allow_back_forward = config.lockdown.allow_back_forward;
    let mut allow_reload = config.lockdown.allow_reload;
    let mut allow_zoom = config.browser.allow_zoom;
    let mut a11y_state = AccessibilitySnapshot {
        allow_read_aloud: config.accessibility.allow_read_aloud,
        read_aloud_rate: AccessibilityPolicy::clamp_rate(config.accessibility.read_aloud_rate),
        read_aloud_voice: config.accessibility.read_aloud_voice.clone(),
        available_voices: Vec::new(),
        high_contrast: config.accessibility.high_contrast,
        reduced_motion: config.accessibility.reduced_motion,
        color_tint: config.accessibility.color_tint,
        line_focus: config.accessibility.line_focus,
        readable_spacing: config.accessibility.readable_spacing,
    };

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::Resized(physical_size),
                ..
            } => {
                let size = physical_size.to_logical::<f64>(window.scale_factor());
                let current_hud_height =
                    if security_blocked || !entrance_unlocked || quit_modal_open || a11y_popover_open
                    {
                        size.height
                    } else {
                        hud_bar_height
                    };
                let _ = hud_webview.set_bounds(Rect {
                    position: LogicalPosition::new(0.0, 0.0).into(),
                    size: LogicalSize::new(size.width, current_hud_height).into(),
                });
                let new_exam_rect = Rect {
                    position: LogicalPosition::new(0.0, hud_bar_height).into(),
                    size: LogicalSize::new(
                        size.width,
                        (size.height - hud_bar_height).max(1.0),
                    )
                    .into(),
                };
                for wv in tab_webviews.values() {
                    let _ = wv.set_bounds(new_exam_rect);
                }
            }
            Event::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } if block_clipboard => {
                let _ = backend.clear_clipboard();
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } if dev_mode
                || exit_authorized
                || !exam_loaded
                || quit_password_hash.is_none() =>
            {
                monitor_stop.store(true, Ordering::Relaxed);
                send_exit_heartbeat(
                    proctor_server_url.as_ref(),
                    &base_config,
                    &config_key,
                    &student_id,
                    &telemetry,
                );
                let _ = lockdown_guard.take();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::HudReady) => {
                sync_hud_state(&hud_webview, &tab_manager);
                sync_hud_accessibility(&hud_webview, &a11y_state);
                let _ = hud_webview.evaluate_script(&format!(
                    "window.__hud && window.__hud.setQuitPasswordRequired && window.__hud.setQuitPasswordRequired({});",
                    quit_password_hash.is_some()
                ));
                let _ = hud_webview.evaluate_script(&format!(
                    "window.__hud && window.__hud.setWebcamRequired && window.__hud.setWebcamRequired({});",
                    require_webcam
                ));
                if !entrance_unlocked {
                    let _ = hud_webview.evaluate_script(
                        "window.__hud && window.__hud.lockEntrance && window.__hud.lockEntrance();",
                    );
                } else {
                    let _ = hud_webview.evaluate_script(
                        "window.__hud && window.__hud.unlockEntrance && window.__hud.unlockEntrance();",
                    );
                }
                if security_blocked
                    && let Ok(findings_json) = serde_json::to_string(&pending_security_findings)
                {
                    let can_exit = !exam_loaded;
                    let _ = hud_webview.evaluate_script(&format!(
                        "window.__hud && window.__hud.showSecurityGate({findings_json}, {can_exit});"
                    ));
                }
            }
            Event::UserEvent(UserEvent::SecurityViolation { findings }) if !dev_mode => {
                security_blocked = true;
                pending_security_findings = findings;
                if let Ok(mut t) = telemetry.lock() {
                    t.status = StudentStatus::SecurityPaused;
                    t.violations = pending_security_findings.clone();
                }
                let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                for wv in tab_webviews.values() {
                    let _ = wv.set_visible(false);
                }
                let _ = hud_webview.set_bounds(Rect {
                    position: LogicalPosition::new(0.0, 0.0).into(),
                    size: LogicalSize::new(size.width, size.height).into(),
                });
                ensure_hud_on_top(&hud_webview);
                if let Ok(findings_json) = serde_json::to_string(&pending_security_findings) {
                    let can_exit = !exam_loaded;
                    let _ = hud_webview.evaluate_script(&format!(
                        "window.__hud && window.__hud.showSecurityGate({findings_json}, {can_exit});"
                    ));
                }
            }
            Event::UserEvent(UserEvent::ProctorForceUnlock) => {
                exit_authorized = true;
                monitor_stop.store(true, Ordering::Relaxed);
                send_exit_heartbeat(
                    proctor_server_url.as_ref(),
                    &base_config,
                    &config_key,
                    &student_id,
                    &telemetry,
                );
                let _ = lockdown_guard.take();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::ProctorPolicyUpdated(policy)) => {
                // Live-update navigation allowlist/blocklist from proctor settings.
                let mut updated_cfg = base_config.clone();
                if !policy.allowed_domains.is_empty() {
                    updated_cfg.allowed_domains = expand_proctor_allowed_domains(
                        &policy.allowed_domains,
                        base_config.start_url.host_str(),
                    );
                } else if let Some(start_host) = base_config.start_url.host_str()
                    && !updated_cfg
                        .allowed_domains
                        .iter()
                        .any(|d| d.eq_ignore_ascii_case(start_host))
                {
                    updated_cfg.allowed_domains.push(start_host.to_string());
                }
                updated_cfg.blocked_domains = policy.blocked_domains.clone();
                if let Ok(mut guard) = url_policy.write() {
                    *guard = UrlPolicy::from_config(&updated_cfg, dev_mode);
                }

                block_downloads_flag.store(policy.block_downloads, Ordering::Relaxed);
                allow_back_forward = policy.allow_back_forward;
                allow_reload = policy.allow_reload;
                allow_zoom = policy.allow_zoom;
                a11y_state.allow_read_aloud = policy.allow_read_aloud;
                require_webcam = policy.require_webcam;
                entrance_code_hash = policy
                    .entrance_code_hash
                    .clone()
                    .or_else(|| base_config.entrance_code_hash.clone());
                quit_password_hash = policy
                    .quit_password_hash
                    .clone()
                    .or_else(|| base_config.quit_password_hash.clone());
                if let Ok(mut sp) = security_policy.lock() {
                    sp.block_multiple_displays = policy.block_multiple_displays;
                    sp.block_screen_capture_apps = policy.block_screen_capture_apps;
                }
                let _ = hud_webview.evaluate_script(&format!(
                    "window.__hud && window.__hud.setQuitPasswordRequired && window.__hud.setQuitPasswordRequired({});",
                    quit_password_hash.is_some()
                ));
                let _ = hud_webview.evaluate_script(&format!(
                    "window.__hud && window.__hud.setWebcamRequired && window.__hud.setWebcamRequired({});",
                    require_webcam
                ));

                let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                if let Some(ref new_hash) = entrance_code_hash {
                    if unlocked_entrance_hash.as_deref() != Some(new_hash.as_str()) {
                        entrance_unlocked = false;
                        a11y_popover_open = false;
                        for wv in tab_webviews.values() {
                            let _ = wv.set_visible(false);
                        }
                        let _ = hud_webview.set_bounds(Rect {
                            position: LogicalPosition::new(0.0, 0.0).into(),
                            size: LogicalSize::new(size.width, size.height).into(),
                        });
                        ensure_hud_on_top(&hud_webview);
                        let _ = hud_webview.evaluate_script(
                            "window.__hud && window.__hud.lockEntrance && window.__hud.lockEntrance();",
                        );
                        if !security_blocked
                            && let Ok(mut t) = telemetry.lock()
                        {
                            t.status = StudentStatus::WaitingForCode;
                        }
                    }
                } else if !entrance_unlocked {
                    entrance_unlocked = true;
                    let _ = hud_webview.evaluate_script(
                        "window.__hud && window.__hud.unlockEntrance && window.__hud.unlockEntrance();",
                    );
                    if !security_blocked {
                        if let Ok(mut t) = telemetry.lock() {
                            t.status = StudentStatus::InExam;
                        }
                        if !quit_modal_open && !a11y_popover_open {
                            let _ = hud_webview.set_bounds(Rect {
                                position: LogicalPosition::new(0.0, 0.0).into(),
                                size: LogicalSize::new(size.width, hud_bar_height).into(),
                            });
                        }
                        let active_id = tab_manager.active_id();
                        if !exam_loaded {
                            exam_loaded = true;
                            for tab in tab_manager.tabs() {
                                if let Some(wv) = tab_webviews.get(&tab.id) {
                                    let _ = wv.load_url(&tab.url);
                                    let _ =
                                        wv.set_visible(!quit_modal_open && tab.id == active_id);
                                }
                            }
                        } else {
                            for (&tid, wv) in &tab_webviews {
                                let _ = wv.set_visible(!quit_modal_open && tid == active_id);
                            }
                        }
                        sync_hud_state(&hud_webview, &tab_manager);
                    }
                }

                // Automatically add/remove live student tabs for teacher-allowed websites.
                let mut desired_domains: Vec<(String, Url)> = Vec::new();
                for pattern in &policy.allowed_domains {
                    if let Some((host, url)) = domain_pattern_to_url(pattern) {
                        let already_initial = tab_manager.tabs().iter().any(|t| {
                            !dynamic_domain_tabs.values().any(|&did| did == t.id)
                                && (t.host.eq_ignore_ascii_case(&host)
                                    || t.host
                                        .to_ascii_lowercase()
                                        .ends_with(&format!(".{host}")))
                        });
                        if !already_initial
                            && !desired_domains.iter().any(|(h, _)| h == &host)
                        {
                            desired_domains.push((host, url));
                        }
                    }
                }

                let mut tabs_changed = false;
                let removed_hosts: Vec<String> = dynamic_domain_tabs
                    .keys()
                    .filter(|h| !desired_domains.iter().any(|(dh, _)| dh == *h))
                    .cloned()
                    .collect();
                for host in removed_hosts {
                    if let Some(tid) = dynamic_domain_tabs.remove(&host) {
                        tab_manager.remove_tab(tid);
                        tab_webviews.remove(&tid);
                        tabs_changed = true;
                    }
                }

                let cur_exam_rect = Rect {
                    position: LogicalPosition::new(0.0, hud_bar_height).into(),
                    size: LogicalSize::new(
                        size.width,
                        (size.height - hud_bar_height).max(1.0),
                    )
                    .into(),
                };
                for (host, url) in desired_domains {
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        dynamic_domain_tabs.entry(host.clone())
                    {
                        let tid = tab_manager.add_tab(host, url.clone(), false);
                        let init_url = if exam_loaded {
                            url.as_str()
                        } else {
                            "about:blank"
                        };
                        if let Ok(wv) = spawn_tab_webview(
                            &window,
                            &proxy,
                            &url_policy,
                            &block_downloads_flag,
                            &init_script,
                            dev_mode,
                            tid,
                            init_url,
                            cur_exam_rect,
                            false,
                            &base_config,
                        ) {
                            let _ = wv.zoom(tab_manager.zoom_level());
                            tab_webviews.insert(tid, wv);
                            entry.insert(tid);
                            tabs_changed = true;
                            ensure_hud_on_top(&hud_webview);
                        }
                    }
                }

                if tabs_changed {
                    let new_hud_h = if tab_manager.has_multiple_tabs() {
                        hud::HUD_WITH_TABS_HEIGHT
                    } else {
                        hud::HUD_BAR_HEIGHT
                    };
                    if (new_hud_h - hud_bar_height).abs() > 0.5 {
                        hud_bar_height = new_hud_h;
                        let current_hud_height = if security_blocked
                            || !entrance_unlocked
                            || quit_modal_open
                            || a11y_popover_open
                        {
                            size.height
                        } else {
                            hud_bar_height
                        };
                        let _ = hud_webview.set_bounds(Rect {
                            position: LogicalPosition::new(0.0, 0.0).into(),
                            size: LogicalSize::new(size.width, current_hud_height).into(),
                        });
                        let resized_exam = Rect {
                            position: LogicalPosition::new(0.0, hud_bar_height).into(),
                            size: LogicalSize::new(
                                size.width,
                                (size.height - hud_bar_height).max(1.0),
                            )
                            .into(),
                        };
                        for wv in tab_webviews.values() {
                            let _ = wv.set_bounds(resized_exam);
                        }
                    }
                    let active_id = tab_manager.active_id();
                    for (&tid, wv) in &tab_webviews {
                        let _ = wv.set_visible(
                            exam_loaded
                                && entrance_unlocked
                                && !security_blocked
                                && !quit_modal_open
                                && tid == active_id,
                        );
                    }
                    sync_hud_state(&hud_webview, &tab_manager);
                }
            }
            Event::UserEvent(UserEvent::CaptureScreenTick) => {
                if exam_loaded
                    && !security_blocked
                    && entrance_unlocked
                    && let Some(tx) = screen_tx.clone()
                    && screen_capture_in_flight
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                {
                    if let Some(wv) = tab_webviews.get(&tab_manager.active_id()) {
                        #[cfg(target_os = "macos")]
                        {
                            use wry::WebViewExtMacOS;

                            let view = wv.webview();
                            let ptr = (&*view as *const _) as *mut std::ffi::c_void;
                            let in_flight = Arc::clone(&screen_capture_in_flight);
                            // SAFETY: `view` is a live `Retained<WryWebView>` (`WKWebView` subclass) on the main thread.
                            unsafe {
                                rustbrowser_platform::snapshot_wkwebview_jpeg(
                                    ptr,
                                    640.0,
                                    0.55,
                                    move |jpeg| {
                                        in_flight.store(false, Ordering::Release);
                                        if let Some(bytes) = jpeg {
                                            let _ = tx.try_send(bytes);
                                        }
                                    },
                                );
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            let _ = (wv, tx);
                            screen_capture_in_flight.store(false, Ordering::Release);
                        }
                    } else {
                        screen_capture_in_flight.store(false, Ordering::Release);
                    }
                }
            }
            Event::UserEvent(UserEvent::PageLoad { tab_id, is_loading }) => {
                tab_manager.set_loading(tab_id, is_loading);
                sync_hud_state(&hud_webview, &tab_manager);
            }
            Event::UserEvent(UserEvent::NavigationAllowed { tab_id, host }) => {
                if tab_id == tab_manager.active_id()
                    && let Ok(mut t) = telemetry.lock()
                {
                    t.active_host = host.clone();
                }
                tab_manager.set_host(tab_id, host);
                sync_hud_state(&hud_webview, &tab_manager);
            }
            Event::UserEvent(UserEvent::NavigationBlocked { reason }) => {
                if let Ok(js_arg) = serde_json::to_string(&reason) {
                    let _ = hud_webview.evaluate_script(&format!(
                        "window.__hud && window.__hud.showBlocked({js_arg});"
                    ));
                }
            }
            Event::UserEvent(UserEvent::ExamIpc { tab_id, msg }) => match msg {
                ExamMessage::TitleChanged { title } if !title.trim().is_empty() => {
                    let capped: String = title.chars().take(160).collect();
                    tab_manager.set_title(tab_id, capped);
                    sync_hud_state(&hud_webview, &tab_manager);
                }
                ExamMessage::VoicesLoaded { mut voices } if a11y_state.allow_read_aloud => {
                    voices.truncate(64);
                    if a11y_state.available_voices != voices {
                        a11y_state.available_voices = voices;
                        sync_hud_accessibility(&hud_webview, &a11y_state);
                    }
                }
                ExamMessage::ZoomIn if allow_zoom => {
                    tab_manager.zoom_in();
                    apply_zoom(&tab_webviews, &hud_webview, &tab_manager);
                }
                ExamMessage::ZoomOut if allow_zoom => {
                    tab_manager.zoom_out();
                    apply_zoom(&tab_webviews, &hud_webview, &tab_manager);
                }
                ExamMessage::ZoomReset if allow_zoom => {
                    tab_manager.zoom_reset();
                    apply_zoom(&tab_webviews, &hud_webview, &tab_manager);
                }
                ExamMessage::DevEmergencyExit if dev_mode => {
                    monitor_stop.store(true, Ordering::Relaxed);
                    send_exit_heartbeat(
                        proctor_server_url.as_ref(),
                        &base_config,
                        &config_key,
                        &student_id,
                        &telemetry,
                    );
                    let _ = lockdown_guard.take();
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            },
            Event::UserEvent(UserEvent::HudIpc(msg)) => match msg {
                HudMessage::SwitchTab { id } if entrance_unlocked => {
                    let target = TabId(id);
                    if tab_manager.activate(target) {
                        for (&tid, wv) in &tab_webviews {
                            let _ = wv.set_visible(!quit_modal_open && tid == target);
                        }
                        if let Ok(mut t) = telemetry.lock() {
                            t.active_host = tab_manager.active_tab().host.clone();
                        }
                        sync_hud_state(&hud_webview, &tab_manager);
                    }
                }
                HudMessage::ReorderTab { from_id, to_id }
                    if tab_manager.reorder_by_id(TabId(from_id), TabId(to_id)) =>
                {
                    sync_hud_state(&hud_webview, &tab_manager);
                }
                HudMessage::ReorderTabs { ids } => {
                    let ordered: Vec<TabId> = ids.into_iter().map(TabId).collect();
                    if tab_manager.set_order(&ordered) {
                        sync_hud_state(&hud_webview, &tab_manager);
                    }
                }
                HudMessage::NavBack => {
                    if allow_back_forward
                        && let Some(wv) = tab_webviews.get(&tab_manager.active_id())
                    {
                        let _ = wv.evaluate_script("history.back();");
                    }
                }
                HudMessage::NavForward => {
                    if allow_back_forward
                        && let Some(wv) = tab_webviews.get(&tab_manager.active_id())
                    {
                        let _ = wv.evaluate_script("history.forward();");
                    }
                }
                HudMessage::NavReload => {
                    if allow_reload
                        && let Some(wv) = tab_webviews.get(&tab_manager.active_id())
                    {
                        let _ = wv.evaluate_script("location.reload();");
                    }
                }
                HudMessage::ZoomIn if allow_zoom => {
                    tab_manager.zoom_in();
                    apply_zoom(&tab_webviews, &hud_webview, &tab_manager);
                }
                HudMessage::ZoomOut if allow_zoom => {
                    tab_manager.zoom_out();
                    apply_zoom(&tab_webviews, &hud_webview, &tab_manager);
                }
                HudMessage::ZoomReset if allow_zoom => {
                    tab_manager.zoom_reset();
                    apply_zoom(&tab_webviews, &hud_webview, &tab_manager);
                }
                HudMessage::ToggleHighContrast => {
                    a11y_state.high_contrast = !a11y_state.high_contrast;
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setHighContrast({});",
                        a11y_state.high_contrast
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::ToggleReducedMotion => {
                    a11y_state.reduced_motion = !a11y_state.reduced_motion;
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setReducedMotion({});",
                        a11y_state.reduced_motion
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::SetColorTint { tint } => {
                    a11y_state.color_tint = tint;
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setColorTint('{}');",
                        tint.as_str()
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::ToggleLineFocus => {
                    a11y_state.line_focus = !a11y_state.line_focus;
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setLineFocus({});",
                        a11y_state.line_focus
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::ToggleReadableSpacing => {
                    a11y_state.readable_spacing = !a11y_state.readable_spacing;
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setReadableSpacing({});",
                        a11y_state.readable_spacing
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::ReadAloudToggle if a11y_state.allow_read_aloud && entrance_unlocked => {
                    if let Some(wv) = tab_webviews.get(&tab_manager.active_id()) {
                        let voice_arg = serde_json::to_string(&a11y_state.read_aloud_voice)
                            .unwrap_or_else(|_| "null".to_string());
                        let _ = wv.evaluate_script(&format!(
                            "window.__examA11y && window.__examA11y.toggleReadAloud({}, {voice_arg});",
                            a11y_state.read_aloud_rate
                        ));
                    }
                }
                HudMessage::ReadAloudStop if a11y_state.allow_read_aloud => {
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(
                            "window.__examA11y && window.__examA11y.stopReadAloud();",
                        );
                    }
                }
                HudMessage::ReadAloudRate { rate } if a11y_state.allow_read_aloud => {
                    a11y_state.read_aloud_rate = AccessibilityPolicy::clamp_rate(rate);
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setRate({});",
                        a11y_state.read_aloud_rate
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::ReadAloudVoice { voice } if a11y_state.allow_read_aloud => {
                    a11y_state.read_aloud_voice = voice.filter(|v| !v.trim().is_empty());
                    let voice_arg = serde_json::to_string(&a11y_state.read_aloud_voice)
                        .unwrap_or_else(|_| "null".to_string());
                    let script = format!(
                        "window.__examA11y && window.__examA11y.setVoice({voice_arg});"
                    );
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(&script);
                    }
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::RequestAccessibilityState => {
                    sync_hud_accessibility(&hud_webview, &a11y_state);
                }
                HudMessage::OpenA11yPopover => {
                    a11y_popover_open = true;
                    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                    let _ = hud_webview.set_bounds(Rect {
                        position: LogicalPosition::new(0.0, 0.0).into(),
                        size: LogicalSize::new(size.width, size.height).into(),
                    });
                    ensure_hud_on_top(&hud_webview);
                }
                HudMessage::CloseA11yPopover => {
                    a11y_popover_open = false;
                    if !security_blocked && entrance_unlocked && !quit_modal_open {
                        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                        let _ = hud_webview.set_bounds(Rect {
                            position: LogicalPosition::new(0.0, 0.0).into(),
                            size: LogicalSize::new(size.width, hud_bar_height).into(),
                        });
                    }
                }
                HudMessage::RecheckSecurity => {
                    let _ = hud_webview.evaluate_script(
                        "window.__hud && window.__hud.retryWebcam && window.__hud.retryWebcam();",
                    );
                    let policy_snap = security_policy.lock().unwrap().clone();
                    let recheck_probes = rustbrowser_platform::default_probes();
                    let report = evaluate_probes(&recheck_probes, &policy_snap);
                    let mut findings: Vec<String> = if !dev_mode && report.has_blocking_findings(&policy_snap) {
                        report
                            .critical_findings()
                            .map(|f| format!("[{}] {}", f.probe, f.message))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    if let Some(reason) = &webcam_denied_reason {
                        findings.push(format!(
                            "[webcam] Camera access is required by your proctor ({reason}). Allow camera access and click Recheck."
                        ));
                    }
                    if !findings.is_empty() {
                        security_blocked = true;
                        pending_security_findings = findings;
                        if let Ok(findings_json) =
                            serde_json::to_string(&pending_security_findings)
                        {
                            let can_exit = !exam_loaded;
                            let _ = hud_webview.evaluate_script(&format!(
                                "window.__hud && window.__hud.showSecurityGate({findings_json}, {can_exit});"
                            ));
                        }
                    } else {
                        security_blocked = false;
                        pending_security_findings.clear();
                        if let Ok(mut t) = telemetry.lock() {
                            t.violations.clear();
                            t.status = if entrance_unlocked {
                                StudentStatus::InExam
                            } else {
                                StudentStatus::WaitingForCode
                            };
                        }
                        let _ = hud_webview.evaluate_script(
                            "window.__hud && window.__hud.hideSecurityGate();",
                        );
                        if entrance_unlocked && !quit_modal_open {
                            let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                            let _ = hud_webview.set_bounds(Rect {
                                position: LogicalPosition::new(0.0, 0.0).into(),
                                size: LogicalSize::new(size.width, hud_bar_height).into(),
                            });
                            let active_id = tab_manager.active_id();
                            if !exam_loaded {
                                exam_loaded = true;
                                for tab in tab_manager.tabs() {
                                    if let Some(wv) = tab_webviews.get(&tab.id) {
                                        let _ = wv.load_url(&tab.url);
                                        let _ = wv.set_visible(tab.id == active_id);
                                    }
                                }
                                sync_hud_state(&hud_webview, &tab_manager);
                            } else {
                                for (&tid, wv) in &tab_webviews {
                                    let _ = wv.set_visible(tid == active_id);
                                }
                            }
                        }
                    }
                }
                HudMessage::CancelPreflight if !exam_loaded => {
                    exit_authorized = true;
                    monitor_stop.store(true, Ordering::Relaxed);
                    send_exit_heartbeat(
                        proctor_server_url.as_ref(),
                        &base_config,
                        &config_key,
                        &student_id,
                        &telemetry,
                    );
                    let _ = lockdown_guard.take();
                    *control_flow = ControlFlow::Exit;
                }
                HudMessage::WebcamFrame { data_url } => {
                    if let Some(tx) = &webcam_tx {
                        let _ = tx.try_send(data_url);
                    }
                }
                HudMessage::WebcamDenied { reason } => {
                    webcam_denied_reason = Some(reason.clone());
                    let msg = format!(
                        "[webcam] Camera access is required by your proctor ({reason}). Allow camera access in macOS Settings and click Recheck."
                    );
                    if !pending_security_findings.iter().any(|f| f.starts_with("[webcam]")) {
                        pending_security_findings.push(msg);
                    }
                    security_blocked = true;
                    a11y_popover_open = false;
                    if let Ok(mut t) = telemetry.lock() {
                        t.status = StudentStatus::SecurityPaused;
                        t.violations = pending_security_findings.clone();
                    }
                    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                    for wv in tab_webviews.values() {
                        let _ = wv.set_visible(false);
                    }
                    let _ = hud_webview.set_bounds(Rect {
                        position: LogicalPosition::new(0.0, 0.0).into(),
                        size: LogicalSize::new(size.width, size.height).into(),
                    });
                    ensure_hud_on_top(&hud_webview);
                    if let Ok(findings_json) = serde_json::to_string(&pending_security_findings) {
                        let can_exit = !exam_loaded;
                        let _ = hud_webview.evaluate_script(&format!(
                            "window.__hud && window.__hud.showSecurityGate({findings_json}, {can_exit});"
                        ));
                    }
                }
                HudMessage::WebcamReady => {
                    webcam_denied_reason = None;
                    pending_security_findings.retain(|f| !f.starts_with("[webcam]"));
                    if security_blocked && pending_security_findings.is_empty() {
                        security_blocked = false;
                        if let Ok(mut t) = telemetry.lock() {
                            t.violations.clear();
                            t.status = if entrance_unlocked {
                                StudentStatus::InExam
                            } else {
                                StudentStatus::WaitingForCode
                            };
                        }
                        let _ = hud_webview.evaluate_script(
                            "window.__hud && window.__hud.hideSecurityGate();",
                        );
                        if entrance_unlocked && !quit_modal_open {
                            let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                            let _ = hud_webview.set_bounds(Rect {
                                position: LogicalPosition::new(0.0, 0.0).into(),
                                size: LogicalSize::new(size.width, hud_bar_height).into(),
                            });
                            let active_id = tab_manager.active_id();
                            if !exam_loaded {
                                exam_loaded = true;
                                for tab in tab_manager.tabs() {
                                    if let Some(wv) = tab_webviews.get(&tab.id) {
                                        let _ = wv.load_url(&tab.url);
                                        let _ = wv.set_visible(tab.id == active_id);
                                    }
                                }
                                sync_hud_state(&hud_webview, &tab_manager);
                            } else {
                                for (&tid, wv) in &tab_webviews {
                                    let _ = wv.set_visible(tid == active_id);
                                }
                            }
                        }
                    }
                }
                HudMessage::OpenQuitModal => {
                    a11y_popover_open = false;
                    quit_modal_open = true;
                    if let Some(wv) = tab_webviews.get(&tab_manager.active_id()) {
                        let _ = wv.evaluate_script(
                            "window.__examA11y && window.__examA11y.setModalBlur(true);",
                        );
                    }
                    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                    let _ = hud_webview.set_bounds(Rect {
                        position: LogicalPosition::new(0.0, 0.0).into(),
                        size: LogicalSize::new(size.width, size.height).into(),
                    });
                    ensure_hud_on_top(&hud_webview);
                }
                HudMessage::CancelQuit => {
                    quit_modal_open = false;
                    for wv in tab_webviews.values() {
                        let _ = wv.evaluate_script(
                            "window.__examA11y && window.__examA11y.setModalBlur(false);",
                        );
                    }
                    if !security_blocked && entrance_unlocked && !a11y_popover_open {
                        let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                        let _ = hud_webview.set_bounds(Rect {
                            position: LogicalPosition::new(0.0, 0.0).into(),
                            size: LogicalSize::new(size.width, hud_bar_height).into(),
                        });
                    }
                }
                HudMessage::QuitNoPassword if quit_password_hash.is_none() => {
                    exit_authorized = true;
                    monitor_stop.store(true, Ordering::Relaxed);
                    send_exit_heartbeat(
                        proctor_server_url.as_ref(),
                        &base_config,
                        &config_key,
                        &student_id,
                        &telemetry,
                    );
                    let _ = lockdown_guard.take();
                    *control_flow = ControlFlow::Exit;
                }
                HudMessage::SubmitQuitPassword { password } => {
                    let ok = quit_password_hash
                        .as_deref()
                        .and_then(|hash| verify_password(&password, hash).ok())
                        .unwrap_or(false);
                    if ok {
                        exit_authorized = true;
                        monitor_stop.store(true, Ordering::Relaxed);
                        send_exit_heartbeat(
                            proctor_server_url.as_ref(),
                            &base_config,
                            &config_key,
                            &student_id,
                            &telemetry,
                        );
                        let _ = lockdown_guard.take();
                        *control_flow = ControlFlow::Exit;
                    } else {
                        let _ = hud_webview.evaluate_script(
                            "window.__hud && window.__hud.showQuitError('Incorrect quit password.');",
                        );
                    }
                }
                HudMessage::SubmitEntranceCode { code } if !entrance_unlocked => {
                    let ok = entrance_code_hash
                        .as_deref()
                        .and_then(|hash| verify_password(&code, hash).ok())
                        .unwrap_or(false);
                    if ok {
                        unlocked_entrance_hash = entrance_code_hash.clone();
                        entrance_unlocked = true;
                        if let Ok(mut t) = telemetry.lock() {
                            t.status = if security_blocked {
                                StudentStatus::SecurityPaused
                            } else {
                                StudentStatus::InExam
                            };
                        }
                        let _ = hud_webview.evaluate_script(
                            "window.__hud && window.__hud.unlockEntrance();",
                        );
                        if !security_blocked && !quit_modal_open {
                            let size = window.inner_size().to_logical::<f64>(window.scale_factor());
                            let _ = hud_webview.set_bounds(Rect {
                                position: LogicalPosition::new(0.0, 0.0).into(),
                                size: LogicalSize::new(size.width, hud_bar_height).into(),
                            });
                            let active_id = tab_manager.active_id();
                            if !exam_loaded {
                                exam_loaded = true;
                                for tab in tab_manager.tabs() {
                                    if let Some(wv) = tab_webviews.get(&tab.id) {
                                        let _ = wv.load_url(&tab.url);
                                        let _ = wv.set_visible(tab.id == active_id);
                                    }
                                }
                            } else {
                                for (&tid, wv) in &tab_webviews {
                                    let _ = wv.set_visible(tid == active_id);
                                }
                            }
                            sync_hud_state(&hud_webview, &tab_manager);
                        }
                    } else {
                        let _ = hud_webview.evaluate_script(
                            "window.__hud && window.__hud.showEntranceError('Incorrect entrance code.');",
                        );
                    }
                }
                HudMessage::CancelEntrance if !entrance_unlocked => {
                    exit_authorized = true;
                    monitor_stop.store(true, Ordering::Relaxed);
                    send_exit_heartbeat(
                        proctor_server_url.as_ref(),
                        &base_config,
                        &config_key,
                        &student_id,
                        &telemetry,
                    );
                    let _ = lockdown_guard.take();
                    *control_flow = ControlFlow::Exit;
                }
                HudMessage::DevEmergencyExit if dev_mode => {
                    monitor_stop.store(true, Ordering::Relaxed);
                    send_exit_heartbeat(
                        proctor_server_url.as_ref(),
                        &base_config,
                        &config_key,
                        &student_id,
                        &telemetry,
                    );
                    let _ = lockdown_guard.take();
                    *control_flow = ControlFlow::Exit;
                }
                _ => {}
            },
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_bare_proctor_domains_and_preserves_start_host() {
        let expanded = expand_proctor_allowed_domains(
            &["desmos.com".to_string(), "*.khanacademy.org".to_string()],
            Some("example.com"),
        );

        assert!(expanded.contains(&"example.com".to_string()));
        assert!(expanded.contains(&"desmos.com".to_string()));
        assert!(expanded.contains(&"*.desmos.com".to_string()));
        assert!(expanded.contains(&"khanacademy.org".to_string()));
        assert!(expanded.contains(&"*.khanacademy.org".to_string()));

        let mut cfg = ExamConfig::for_url(Url::parse("https://example.com/").unwrap()).unwrap();
        cfg.allowed_domains = expanded;
        let policy = UrlPolicy::from_config(&cfg, false);

        assert_eq!(
            policy.evaluate_str("https://example.com/"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://desmos.com/"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://www.desmos.com/calculator"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://student.desmos.com/"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://khanacademy.org/"),
            NavigationDecision::Allow
        );
        assert!(matches!(
            policy.evaluate_str("https://evil-desmos.com/"),
            NavigationDecision::Block(_)
        ));
    }

    #[test]
    fn hud_loopback_server_requires_secret_token_and_get_method() {
        let html = "<!doctype html><h1>HUD</h1>".to_string();
        let url_str = start_hud_loopback_server(html.clone()).expect("server should bind");
        let parsed = Url::parse(&url_str).expect("valid loopback URL");
        let addr = format!(
            "{}:{}",
            parsed.host_str().unwrap(),
            parsed.port().unwrap()
        );
        let path = parsed.path();
        assert_eq!(path.len(), 33, "expected '/' + 32 hex chars");

        let send_raw = |req: &str| -> String {
            let mut s = std::net::TcpStream::connect(&addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            s.write_all(req.as_bytes()).unwrap();
            s.flush().unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).unwrap();
            resp
        };

        let ok = send_raw(&format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n"));
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(ok.ends_with(&html));

        let wrong_path = send_raw(&format!("GET / HTTP/1.1\r\nHost: {addr}\r\n\r\n"));
        assert!(wrong_path.starts_with("HTTP/1.1 404 Not Found\r\n"));

        let wrong_method = send_raw(&format!("POST {path} HTTP/1.1\r\nHost: {addr}\r\n\r\n"));
        assert!(wrong_method.starts_with("HTTP/1.1 404 Not Found\r\n"));
    }
}
