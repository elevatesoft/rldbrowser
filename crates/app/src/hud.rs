use rustbrowser_core::ExamConfig;

pub const HUD_BAR_HEIGHT: f64 = 40.0;
pub const HUD_WITH_TABS_HEIGHT: f64 = 74.0;

#[cfg(test)]
pub fn build_hud_html(config: &ExamConfig, dev_mode: bool, kiosk_active: bool) -> String {
    build_hud_html_with_options(config, dev_mode, kiosk_active, false, true)
}

pub fn build_hud_html_with_options(
    config: &ExamConfig,
    dev_mode: bool,
    kiosk_active: bool,
    has_tabs: bool,
    allow_zoom: bool,
) -> String {
    let exam_name = escape_html(&config.exam_name);
    let initial_host = escape_html(config.start_url.host_str().unwrap_or(""));
    let has_password = config.quit_password_hash.is_some();
    let requires_entrance = config.requires_entrance_code();
    let entrance_open_class = if requires_entrance { " open" } else { "" };
    let allow_back_forward = config.lockdown.allow_back_forward;
    let allow_reload = config.lockdown.allow_reload;
    let allow_read_aloud = config.accessibility.allow_read_aloud;
    let enable_webcam = config.proctor.enable_webcam && config.proctor.server_url.is_some();
    let hud_height = if has_tabs {
        HUD_WITH_TABS_HEIGHT
    } else {
        HUD_BAR_HEIGHT
    };

    let nav_buttons = if allow_back_forward {
        r##"<button type="button" class="icon-btn" id="back-btn" title="Back" aria-label="Back">
            <svg viewBox="0 0 24 24" width="16" height="16" shape-rendering="geometricPrecision"><path d="m15 18-6-6 6-6" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round"/></svg>
          </button>
          <button type="button" class="icon-btn" id="forward-btn" title="Forward" aria-label="Forward">
            <svg viewBox="0 0 24 24" width="16" height="16" shape-rendering="geometricPrecision"><path d="m9 18 6-6-6-6" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round"/></svg>
          </button>"##
    } else {
        ""
    };

    let reload_btn = if allow_reload {
        r##"<button type="button" class="icon-btn" id="reload-btn" title="Reload" aria-label="Reload">
            <svg viewBox="0 0 24 24" width="15" height="15" shape-rendering="geometricPrecision"><path d="M21 12a9 9 0 1 1-2.64-6.36L21 8" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round"/><path d="M21 3v5h-5" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round" stroke-linejoin="round"/></svg>
          </button>"##
    } else {
        ""
    };

    let nav_divider = if (allow_back_forward || allow_reload) && allow_zoom {
        r#"<span class="bar-divider" aria-hidden="true"></span>"#
    } else {
        ""
    };

    let zoom_controls = if allow_zoom {
        r##"<div class="zoom-group" role="group" aria-label="Page zoom">
            <button type="button" class="icon-btn" id="zoom-out-btn" title="Zoom out" aria-label="Zoom out">
              <svg viewBox="0 0 24 24" width="15" height="15" shape-rendering="geometricPrecision"><path d="M5 12h14" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round"/></svg>
            </button>
            <button type="button" class="zoom-value" id="zoom-reset-btn" title="Reset zoom" aria-label="Reset zoom to 100%">100%</button>
            <button type="button" class="icon-btn" id="zoom-in-btn" title="Zoom in" aria-label="Zoom in">
              <svg viewBox="0 0 24 24" width="15" height="15" shape-rendering="geometricPrecision"><path d="M12 5v14M5 12h14" fill="none" stroke="currentColor" stroke-width="1.85" stroke-linecap="round"/></svg>
            </button>
          </div>"##
    } else {
        ""
    };

    let tab_strip_html = if has_tabs {
        r#"<div class="tab-strip" id="tab-strip" role="tablist" aria-label="Exam materials"></div>"#
    } else {
        ""
    };

    let version = env!("CARGO_PKG_VERSION");

    let read_aloud_section = if allow_read_aloud {
        r#"<div class="pop-divider"></div>
        <div class="pop-section">Read Aloud</div>
        <div class="pop-row">
          <div class="pop-label" id="a11y-ra-status">Ready</div>
          <div class="ra-actions">
            <button type="button" class="btn-primary btn-sm" id="a11y-ra-toggle">Play</button>
            <button type="button" class="btn-secondary btn-sm" id="a11y-ra-stop">Stop</button>
          </div>
        </div>
        <div class="pop-row">
          <div>
            <div class="pop-label">Speed</div>
            <div class="pop-sub">0.5×–2.0×</div>
          </div>
          <div class="stepper" role="group" aria-label="Read aloud speed">
            <button type="button" class="stepper-btn" id="a11y-rate-down" aria-label="Decrease speed">−</button>
            <span class="stepper-val" id="a11y-rate-val">1×</span>
            <button type="button" class="stepper-btn" id="a11y-rate-up" aria-label="Increase speed">+</button>
          </div>
        </div>"#
    } else {
        ""
    };

    let quit_pwd_display = if has_password { "" } else { " style=\"display:none;\"" };
    let quit_title_text = if has_password { "Quit exam?" } else { "End exam?" };
    let quit_desc_text = if has_password {
        "Enter the quit password to leave this exam."
    } else {
        "Are you sure you want to end your exam session?"
    };
    let quit_submit_text = if has_password { "Unlock &amp; Quit" } else { "Exit Exam" };
    let quit_modal_body = format!(
        r#"<div class="modal-body">
        <h2 id="quit-title">{quit_title_text}</h2>
        <p class="modal-desc" id="quit-desc">{quit_desc_text}</p>
        <div id="quit-pwd-wrap"{quit_pwd_display}>
          <label class="field-label" for="quit-password">Quit password</label>
          <input type="text" class="field-input" id="quit-password" placeholder="Enter quit password" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false" aria-label="Quit password" />
          <div class="field-error" id="quit-error" role="alert" aria-live="assertive"></div>
        </div>
      </div>
      <div class="modal-footer">
        <button type="button" class="btn-secondary" id="cancel-quit">Cancel</button>
        <button type="submit" class="btn-danger" id="quit-submit-btn">{quit_submit_text}</button>
      </div>"#
    );

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<style>
  :root {{
    --bg-chrome: #F4F6F9;
    --bg-surface: #FFFFFF;
    --bg-subtle: #F5F7FA;
    --bg-hover: #E7ECF3;
    --border: #D7DFE8;
    --border-popover: #8B98A8;
    --text-primary: #192230;
    --text-secondary: #4A5566;
    --text-muted: #8994A4;
    --accent: #235DBC;
    --accent-hover: #1D4FA0;
    --accent-soft: #E4ECFA;
    --danger-bar: #AD3439;
    --danger-bar-hover: #962A2F;
    --danger: #A62B36;
    --danger-hover: #8F222C;
    --alert-label: #91323D;
    --alert-bar: #B45962;
    --switch-off: #C7D0DE;
    --backdrop: rgba(51, 66, 86, 0.15);
    --font: "SF Pro", -apple-system, BlinkMacSystemFont, "Helvetica Neue", system-ui, sans-serif;
    --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace;
  }}

  * {{
    box-sizing: border-box;
    margin: 0;
    padding: 0;
    user-select: none;
    -webkit-user-select: none;
    -webkit-font-smoothing: antialiased;
  }}

  html, body {{
    width: 100%;
    height: 100%;
    overflow: hidden;
    font-family: var(--font);
    background: transparent;
    color: var(--text-primary);
  }}

  button:focus-visible, input:focus-visible {{
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }}

  /* Top Chrome (Quiet Native) */
  .hud-root {{
    position: relative;
    width: 100%;
    height: {hud_height}px;
    background: var(--bg-chrome);
    border-bottom: 1px solid var(--border);
    z-index: 20;
  }}

  .bar {{
    height: {HUD_BAR_HEIGHT}px;
    padding: 0 14px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }}

  .left {{
    display: flex;
    flex-direction: column;
    justify-content: center;
    min-width: 0;
    max-width: 300px;
  }}
  .title-row {{
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }}
  .title {{
    font-size: 13px;
    font-weight: 590;
    line-height: 17px;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }}
  .ver-pill {{
    font-family: var(--mono);
    font-size: 10.5px;
    font-weight: 500;
    line-height: 14px;
    color: var(--text-muted);
    background: rgba(25, 34, 48, 0.05);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 1px 6px;
    letter-spacing: 0.01em;
    flex-shrink: 0;
  }}
  .host {{
    font-size: 11px;
    line-height: 14px;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }}

  .center {{
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-width: 0;
  }}
  .notice {{
    display: none;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 510;
    line-height: 16px;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 420px;
  }}
  .notice.visible {{ display: inline-flex; }}

  .right {{
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }}

  .icon-btn {{
    appearance: none;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-secondary);
    width: 28px;
    height: 28px;
    border-radius: 5px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background 100ms ease, color 100ms ease;
  }}
  .icon-btn:hover {{
    background: var(--bg-hover);
    color: var(--text-primary);
  }}
  .icon-btn:disabled {{
    color: var(--text-muted);
    cursor: default;
  }}
  .icon-btn:disabled:hover {{ background: transparent; }}

  .bar-divider {{
    width: 1px;
    height: 16px;
    background: var(--border);
    margin: 0 6px;
  }}

  .zoom-group {{
    display: inline-flex;
    align-items: center;
    gap: 0;
  }}
  .zoom-value {{
    appearance: none;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 12px;
    font-weight: 510;
    min-width: 38px;
    height: 28px;
    border-radius: 5px;
    text-align: center;
    cursor: pointer;
  }}
  .zoom-value:hover {{
    background: var(--bg-hover);
    color: var(--text-primary);
  }}

  .a11y-trigger {{
    appearance: none;
    width: 28px;
    height: 28px;
    margin-left: 4px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: #1F242A;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background 100ms ease, border-color 100ms ease;
  }}
  .a11y-trigger:hover {{
    background: #F8FAFC;
    border-color: #B8C4D4;
  }}
  .a11y-trigger.open {{
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--accent);
  }}

  .quit-btn {{
    appearance: none;
    margin-left: 8px;
    height: 28px;
    padding: 0 11px;
    border-radius: 7px;
    border: 1px solid var(--danger-bar);
    background: var(--danger-bar);
    color: #FFFFFF;
    font-family: var(--font);
    font-size: 12px;
    font-weight: 590;
    line-height: 16px;
    cursor: pointer;
    transition: background 100ms ease;
  }}
  .quit-btn:hover {{
    background: var(--danger-bar-hover);
    border-color: var(--danger-bar-hover);
  }}

  /* Pinned Materials Tab Strip (34px) */
  .tab-strip {{
    height: 34px;
    padding: 4px 14px 0 14px;
    border-top: 1px solid var(--border);
    display: flex;
    align-items: flex-end;
    gap: 4px;
    background: var(--bg-chrome);
    overflow-x: auto;
    overflow-y: hidden;
  }}
  .tab-strip::-webkit-scrollbar {{ display: none; }}

  .tab-pill {{
    appearance: none;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 29px;
    max-width: 224px;
    padding: 0 12px 1px 11px;
    border-top-left-radius: 7px;
    border-top-right-radius: 7px;
    border: 1px solid transparent;
    border-bottom: none;
    background: transparent;
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 12px;
    font-weight: 510;
    line-height: 16px;
    cursor: pointer;
    flex-shrink: 0;
    position: relative;
    bottom: -1px;
    transition: background 120ms ease, color 120ms ease, border-color 120ms ease;
  }}
  .tab-pill:hover {{
    background: var(--bg-hover);
    color: var(--text-primary);
  }}
  .tab-pill.active {{
    background: var(--bg-surface);
    border-color: var(--border);
    color: var(--text-primary);
    font-weight: 590;
  }}
  .tab-pill.dragging {{
    position: relative;
    z-index: 10;
    background: var(--bg-surface);
    border-color: var(--border);
    box-shadow: 0 3px 10px rgba(25, 34, 48, 0.14);
    cursor: grabbing;
  }}
  [data-tab-icon] {{
    width: 15px;
    height: 15px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    color: var(--text-muted);
    transition: color 120ms ease;
  }}
  [data-tab-icon] svg {{
    display: block;
    width: 14px;
    height: 14px;
    flex-shrink: 0;
  }}
  .tab-pill:hover [data-tab-icon] {{
    color: var(--text-secondary);
  }}
  .tab-pill.active [data-tab-icon] {{
    color: var(--accent);
  }}
  .tab-label {{
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 16px;
  }}
  .tab-spinner {{
    display: block;
    width: 13px;
    height: 13px;
    border-radius: 999px;
    border: 1.75px solid #D7DFE8;
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
    flex-shrink: 0;
  }}
  @keyframes spin {{
    to {{ transform: rotate(360deg); }}
  }}

  /* 2px Loading Progress Bar */
  .loading-bar {{
    position: absolute;
    left: 0;
    right: 0;
    bottom: -1px;
    height: 2px;
    overflow: hidden;
    opacity: 0;
    pointer-events: none;
    transition: opacity 120ms ease;
  }}
  .loading-bar.active {{ opacity: 1; }}
  .loading-bar::before {{
    content: "";
    position: absolute;
    top: 0;
    left: -35%;
    width: 35%;
    height: 100%;
    background: linear-gradient(90deg, transparent, var(--accent), #4E86DF, transparent);
    animation: hud-loading 1.1s linear infinite;
  }}
  @keyframes hud-loading {{
    0% {{ left: -35%; }}
    100% {{ left: 100%; }}
  }}

  /* Shared Buttons & Inputs */
  .btn-primary {{
    appearance: none;
    height: 32px;
    padding: 0 14px;
    border-radius: 7px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: #FFFFFF;
    font-family: var(--font);
    font-size: 12px;
    font-weight: 590;
    cursor: pointer;
  }}
  .btn-primary:hover {{ background: var(--accent-hover); border-color: var(--accent-hover); }}

  .btn-secondary {{
    appearance: none;
    height: 32px;
    padding: 0 14px;
    border-radius: 7px;
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: var(--text-primary);
    font-family: var(--font);
    font-size: 12px;
    font-weight: 590;
    cursor: pointer;
  }}
  .btn-secondary:hover {{ background: var(--bg-subtle); }}

  .btn-danger {{
    appearance: none;
    height: 32px;
    padding: 0 14px;
    border-radius: 7px;
    border: 1px solid var(--danger);
    background: var(--danger);
    color: #FFFFFF;
    font-family: var(--font);
    font-size: 12px;
    font-weight: 590;
    cursor: pointer;
  }}
  .btn-danger:hover {{ background: var(--danger-hover); border-color: var(--danger-hover); }}

  .btn-sm {{
    height: 30px;
    min-width: 66px;
    padding: 0 12px;
  }}

  /* Accessibility Floating Popover (transparent backdrop over live exam page) */
  .a11y-backdrop {{
    display: none;
    position: fixed;
    inset: 0;
    background: transparent;
    z-index: 30;
  }}
  .a11y-backdrop.open {{ display: block; }}

  .a11y-popover {{
    position: fixed;
    top: 42px;
    right: 108px;
    width: 328px;
    background: var(--bg-surface);
    border: 1px solid var(--border-popover);
    border-radius: 9px;
    box-shadow: 0 9px 23px rgba(34, 49, 74, 0.14);
    color: var(--text-primary);
    overflow: hidden;
  }}
  .pop-header {{
    padding: 12px 16px;
    font-size: 17px;
    font-weight: 590;
    line-height: 22px;
    border-bottom: 1px solid var(--border);
  }}
  .pop-content {{
    padding: 12px 16px 16px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }}
  .pop-section {{
    font-size: 12px;
    font-weight: 590;
    line-height: 17px;
    color: var(--text-secondary);
  }}
  .pop-row {{
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }}
  .pop-label {{
    font-size: 13px;
    font-weight: 590;
    line-height: 18px;
    color: var(--text-primary);
  }}
  .pop-sub {{
    font-size: 11px;
    line-height: 15px;
    color: var(--text-secondary);
  }}
  .pop-divider {{
    height: 1px;
    background: var(--border);
  }}

  .stepper {{
    display: inline-flex;
    align-items: center;
    height: 28px;
    background: var(--bg-chrome);
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }}
  .stepper-btn {{
    appearance: none;
    width: 28px;
    height: 28px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 15px;
    font-weight: 590;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }}
  .stepper-btn:hover {{ background: var(--bg-hover); }}
  .stepper-val {{
    min-width: 50px;
    text-align: center;
    font-size: 12px;
    font-weight: 590;
    color: var(--text-primary);
  }}

  .switch-wrap {{
    display: inline-flex;
    align-items: center;
    gap: 8px;
  }}
  .switch-state {{
    font-size: 11px;
    font-weight: 510;
    color: var(--text-secondary);
    min-width: 22px;
    text-align: right;
  }}
  .switch {{
    appearance: none;
    position: relative;
    width: 34px;
    height: 20px;
    border-radius: 11px;
    border: none;
    background: var(--switch-off);
    cursor: pointer;
    transition: background 140ms ease;
    padding: 0;
  }}
  .switch::after {{
    content: "";
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 999px;
    background: #FFFFFF;
    transition: transform 140ms ease;
  }}
  .switch[aria-checked="true"] {{
    background: var(--accent);
  }}
  .switch[aria-checked="true"]::after {{
    transform: translateX(14px);
  }}

  .segmented {{
    position: relative;
    display: grid;
    grid-template-columns: 76px 64px 1fr;
    gap: 2px;
    padding: 1px;
    height: 30px;
    background: var(--bg-chrome);
    border: 1px solid var(--border);
    border-radius: 6px;
  }}
  .seg-thumb {{
    position: absolute;
    top: 1px;
    left: 0;
    height: 26px;
    border-radius: 5px;
    background: var(--accent-soft);
    pointer-events: none;
    transition: transform 220ms cubic-bezier(0.22, 1, 0.36, 1), width 220ms cubic-bezier(0.22, 1, 0.36, 1);
    z-index: 0;
  }}
  .reduced-motion .seg-thumb,
  .reduced-motion .switch,
  .reduced-motion .switch::after {{
    transition: none !important;
  }}
  .seg-btn {{
    position: relative;
    z-index: 1;
    appearance: none;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 11.5px;
    font-weight: 510;
    cursor: pointer;
    white-space: nowrap;
    transition: color 140ms ease;
  }}
  .seg-btn:hover {{ color: var(--text-primary); }}
  .seg-btn.active {{
    color: var(--text-primary);
    font-weight: 590;
  }}

  .ra-actions {{
    display: inline-flex;
    gap: 8px;
  }}

  /* Full-canvas Gates (Enter Access Code & Preflight Security Setup) */
  .canvas-gate {{
    display: none;
    position: fixed;
    inset: {hud_height}px 0 0 0;
    background: var(--bg-surface);
    align-items: center;
    justify-content: center;
    padding: 24px;
    z-index: 40;
  }}
  .canvas-gate.open {{ display: flex; }}

  .gate-container {{
    width: 100%;
    max-width: 560px;
    transform: translateY(-24px);
  }}
  .gate-title {{
    font-size: 30px;
    font-weight: 590;
    letter-spacing: -0.015em;
    line-height: 38px;
    color: var(--text-primary);
    margin-bottom: 10px;
  }}
  .gate-sub {{
    font-size: 14px;
    line-height: 21px;
    color: var(--text-secondary);
    margin-bottom: 32px;
  }}

  .field-label {{
    display: block;
    font-size: 13px;
    font-weight: 590;
    line-height: 18px;
    color: var(--text-primary);
    margin-bottom: 8px;
  }}
  .field-input {{
    width: 100%;
    height: 40px;
    padding: 0 12px;
    border-radius: 7px;
    border: 2px solid var(--border);
    background: var(--bg-surface);
    color: var(--text-primary);
    font-family: var(--font);
    font-size: 14px;
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }}
  .field-input:focus {{ border-color: var(--accent); }}
  .field-input.error {{ border-color: var(--danger); }}
  .field-error {{
    min-height: 20px;
    margin-top: 8px;
    font-size: 13px;
    font-weight: 510;
    line-height: 18px;
    color: var(--danger);
  }}
  .gate-actions {{
    margin-top: 20px;
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }}

  /* Preflight Card */
  .preflight-card {{
    border: 1px solid var(--border);
    border-radius: 9px;
    overflow: hidden;
    background: var(--bg-surface);
  }}
  .preflight-issues {{
    list-style: none;
    display: flex;
    flex-direction: column;
  }}
  .preflight-issue {{
    position: relative;
    padding: 14px 18px 14px 19px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    border-bottom: 1px solid var(--border);
  }}
  .preflight-issue::before {{
    content: "";
    position: absolute;
    left: 0;
    top: 14px;
    bottom: 14px;
    width: 2px;
    border-radius: 1px;
    background: var(--alert-bar);
  }}
  .issue-kicker {{
    font-size: 11px;
    font-weight: 590;
    line-height: 15px;
    color: var(--alert-label);
    margin-bottom: 3px;
  }}
  .issue-title {{
    font-size: 14px;
    font-weight: 590;
    line-height: 20px;
    color: var(--text-primary);
    margin-bottom: 2px;
  }}
  .issue-desc {{
    font-size: 13px;
    line-height: 19px;
    color: var(--text-secondary);
  }}
  .preflight-passed {{
    padding: 14px 18px;
    background: var(--bg-subtle);
  }}
  .passed-title {{
    font-size: 13px;
    font-weight: 590;
    line-height: 17px;
    color: var(--text-primary);
    margin-bottom: 4px;
  }}
  .passed-list {{
    font-size: 13px;
    line-height: 18px;
    color: var(--text-secondary);
  }}

  /* Blurred Quit Exam Modal */
  .modal-backdrop {{
    display: none;
    position: fixed;
    inset: 0;
    background: var(--backdrop);
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
    align-items: center;
    justify-content: center;
    z-index: 50;
  }}
  .modal-backdrop.open {{ display: flex; }}

  .modal-card {{
    width: 384px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 18px 44px rgba(25, 34, 48, 0.14);
    overflow: hidden;
  }}
  .modal-body {{
    padding: 24px 24px 18px 24px;
  }}
  .modal-body h2 {{
    font-size: 22px;
    font-weight: 590;
    line-height: 28px;
    color: var(--text-primary);
    margin-bottom: 10px;
  }}
  .modal-desc {{
    font-size: 14px;
    line-height: 21px;
    color: var(--text-secondary);
    margin-bottom: 16px;
  }}
  .modal-body .field-input {{
    height: 36px;
  }}
  .modal-footer {{
    padding: 14px 24px;
    border-top: 1px solid var(--border);
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }}
</style>
</head>
<body>
  <div class="hud-root">
    <div class="bar">
      <div class="left">
        <div class="title-row">
          <span class="title">{exam_name}</span>
          <span class="ver-pill" title="RLD Browser version">v{version}</span>
        </div>
        <span class="host" id="host">{initial_host}</span>
      </div>

      <div class="center">
        <span class="notice" id="toast" role="status" aria-live="polite">
          <svg viewBox="0 0 20 20" width="15" height="15" aria-hidden="true"><path d="M10 3.5 17 16H3L10 3.5Zm0 4.5v3.5m0 2.3v.2" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
          <span id="toast-text"></span>
        </span>
      </div>

      <div class="right">
        {nav_buttons}
        {reload_btn}
        {nav_divider}
        {zoom_controls}
        <button type="button" class="a11y-trigger" id="a11y-btn" title="Accessibility" aria-label="Accessibility" aria-expanded="false" aria-controls="a11y-popover">
          <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
            <circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="1.9"/>
            <circle cx="12" cy="7.3" r="1.55" fill="currentColor"/>
            <path d="M6.7 9.7l5.3 1.65 5.3-1.65M12 11.4v2.45m0-.05-3.35 4.2m3.35-4.2 3.35 4.2" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
        <button type="button" class="quit-btn" id="quit-btn">Quit Exam</button>
      </div>
    </div>
    {tab_strip_html}
    <div class="loading-bar" id="loading-bar"></div>
  </div>

  <!-- Accessibility Popover (transparent backdrop over live exam page) -->
  <div class="a11y-backdrop" id="a11y-backdrop">
    <div class="a11y-popover" id="a11y-popover" role="dialog" aria-label="Accessibility settings">
      <div class="pop-header">Accessibility</div>
      <div class="pop-content">
        <div class="pop-section">Display</div>
        <div class="pop-row">
          <div>
            <div class="pop-label">Zoom</div>
            <div class="pop-sub">50%–200%</div>
          </div>
          <div class="stepper" role="group" aria-label="Page zoom">
            <button type="button" class="stepper-btn" id="a11y-zoom-out" aria-label="Zoom out">−</button>
            <span class="stepper-val" id="a11y-zoom-val">100%</span>
            <button type="button" class="stepper-btn" id="a11y-zoom-in" aria-label="Zoom in">+</button>
          </div>
        </div>
        <div class="pop-divider"></div>
        <div class="pop-row">
          <div class="pop-label">High Contrast</div>
          <div class="switch-wrap">
            <span class="switch-state" id="a11y-hc-label">Off</span>
            <button type="button" class="switch" id="a11y-hc-switch" role="switch" aria-checked="false" aria-label="High Contrast"></button>
          </div>
        </div>
        <div class="pop-row">
          <div class="pop-label">Reduced Motion</div>
          <div class="switch-wrap">
            <span class="switch-state" id="a11y-rm-label">Off</span>
            <button type="button" class="switch" id="a11y-rm-switch" role="switch" aria-checked="false" aria-label="Reduced Motion"></button>
          </div>
        </div>

        <div class="pop-divider"></div>
        <div class="pop-section">Reading</div>
        <div>
          <div class="pop-label" style="margin-bottom: 6px;">Reading Tint</div>
          <div class="segmented" id="tint-segmented" role="radiogroup" aria-label="Reading Tint">
            <div class="seg-thumb" id="tint-thumb" aria-hidden="true"></div>
            <button type="button" class="seg-btn active" data-tint="default" aria-pressed="true">Default</button>
            <button type="button" class="seg-btn" data-tint="warm" aria-pressed="false">Warm</button>
            <button type="button" class="seg-btn" data-tint="dark" aria-pressed="false">High-Contrast Dark</button>
          </div>
        </div>
        <div class="pop-row">
          <div class="pop-label">Line Focus</div>
          <div class="switch-wrap">
            <span class="switch-state" id="a11y-lf-label">Off</span>
            <button type="button" class="switch" id="a11y-lf-switch" role="switch" aria-checked="false" aria-label="Line Focus"></button>
          </div>
        </div>
        <div class="pop-row">
          <div class="pop-label">Readable Text Spacing</div>
          <div class="switch-wrap">
            <span class="switch-state" id="a11y-rs-label">Off</span>
            <button type="button" class="switch" id="a11y-rs-switch" role="switch" aria-checked="false" aria-label="Readable Text Spacing"></button>
          </div>
        </div>
        {read_aloud_section}
      </div>
    </div>
  </div>

  <!-- Preflight / Mid-Exam Security Check Gate -->
  <div class="canvas-gate" id="security-modal" role="dialog" aria-modal="true" aria-labelledby="security-title">
    <div class="gate-container">
      <h1 class="gate-title" id="security-title">Check your setup</h1>
      <p class="gate-sub" id="security-desc">Resolve the item(s) below before starting the exam.</p>
      <div class="preflight-card">
        <ul class="preflight-issues" id="security-list" role="alert" aria-live="assertive"></ul>
        <div class="preflight-passed">
          <div class="passed-title" id="security-passed-title">Other checks passed</div>
          <div class="passed-list" id="security-passed-list">Lockdown environment verified</div>
        </div>
      </div>
      <div class="gate-actions">
        <button type="button" class="btn-secondary" id="security-teacher-quit" style="display:none;margin-right:auto;">Teacher Quit</button>
        <button type="button" class="btn-secondary" id="cancel-preflight">Exit</button>
        <button type="button" class="btn-primary" id="recheck-security">Recheck</button>
      </div>
    </div>
  </div>

  <!-- Enter Access Code Gate -->
  <div class="canvas-gate{entrance_open_class}" id="entrance-modal" role="dialog" aria-modal="true" aria-labelledby="entrance-title">
    <div class="gate-container">
      <h1 class="gate-title" id="entrance-title">Enter access code</h1>
      <p class="gate-sub">Enter the access code provided by your proctor.</p>
      <form id="entrance-form">
        <label class="field-label" for="entrance-code">Access code</label>
        <input type="text" class="field-input" id="entrance-code" placeholder="Enter code" autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck="false" aria-label="Access code" />
        <div class="field-error" id="entrance-error" role="alert" aria-live="assertive"></div>
        <div class="gate-actions">
          <button type="button" class="btn-secondary" id="cancel-entrance">Exit</button>
          <button type="submit" class="btn-primary">Confirm code</button>
        </div>
      </form>
    </div>
  </div>

  <!-- Quit Exam Modal (Password or Confirmation variant) -->
  <div class="modal-backdrop" id="quit-modal" role="dialog" aria-modal="true" aria-labelledby="quit-title">
    <form class="modal-card" id="quit-form">
      {quit_modal_body}
    </form>
  </div>

<script>
  let hasQuitPassword = {has_password};
  const REQUIRES_ENTRANCE = {requires_entrance};
  const ENABLE_WEBCAM = {enable_webcam};
  const DEV_MODE = {dev_mode};
  const KIOSK_ACTIVE = {kiosk_active};
  const send = (payload) => window.ipc.postMessage(JSON.stringify(payload));

  const toastEl = document.getElementById('toast');
  const toastTextEl = document.getElementById('toast-text');
  const hostEl = document.getElementById('host');
  const loadingBarEl = document.getElementById('loading-bar');
  const zoomResetBtn = document.getElementById('zoom-reset-btn');
  const tabStripEl = document.getElementById('tab-strip');
  const a11yBtn = document.getElementById('a11y-btn');
  const a11yBackdrop = document.getElementById('a11y-backdrop');
  const a11yPopover = document.getElementById('a11y-popover');
  const a11yZoomVal = document.getElementById('a11y-zoom-val');
  const tintThumb = document.getElementById('tint-thumb');
  const entranceModalEl = document.getElementById('entrance-modal');
  const entranceCodeEl = document.getElementById('entrance-code');
  const entranceErrEl = document.getElementById('entrance-error');
  const securityModalEl = document.getElementById('security-modal');
  const securityListEl = document.getElementById('security-list');
  const cancelPreflightBtn = document.getElementById('cancel-preflight');
  const securityTeacherQuitBtn = document.getElementById('security-teacher-quit');
  const quitModalEl = document.getElementById('quit-modal');
  const pwdEl = document.getElementById('quit-password');
  const errEl = document.getElementById('quit-error');

  let toastTimer = null;
  let a11yOpen = false;
  let raPlaying = false;
  let currentRate = 1.0;

  // Delayed password masking: reveals the last typed character for 1.5s, then masks to '•'.
  const attachDelayedMask = (inputEl, delayMs = 1500) => {{
    if (!inputEl) return {{ getValue: () => '', clear: () => {{}}, maskNow: () => {{}} }};
    let realValue = '';
    let revealIndex = -1;
    let hideTimer = null;

    const render = (caretPos) => {{
      let display = '';
      for (let i = 0; i < realValue.length; i++) {{
        display += i === revealIndex ? realValue[i] : '•';
      }}
      inputEl.value = display;
      if (typeof caretPos === 'number') {{
        try {{ inputEl.setSelectionRange(caretPos, caretPos); }} catch (_) {{}}
      }}
    }};

    const maskAll = () => {{
      clearTimeout(hideTimer);
      hideTimer = null;
      if (revealIndex !== -1) {{
        const pos = inputEl.selectionStart;
        revealIndex = -1;
        render(typeof pos === 'number' ? pos : realValue.length);
      }}
    }};

    const scheduleMask = () => {{
      clearTimeout(hideTimer);
      hideTimer = setTimeout(maskAll, delayMs);
    }};

    inputEl.addEventListener('beforeinput', (e) => {{
      inputEl.classList.remove('error');
      const start = inputEl.selectionStart ?? realValue.length;
      const end = inputEl.selectionEnd ?? realValue.length;
      const t = e.inputType;

      if (t === 'insertText' || t === 'insertFromPaste' || t === 'insertReplacementText') {{
        e.preventDefault();
        const inserted = e.data ?? '';
        realValue = realValue.slice(0, start) + inserted + realValue.slice(end);
        const nextPos = start + inserted.length;
        revealIndex = inserted.length > 0 ? nextPos - 1 : -1;
        render(nextPos);
        if (revealIndex !== -1) scheduleMask();
      }} else if (t === 'deleteContentBackward') {{
        e.preventDefault();
        let nextPos = start;
        if (start !== end) {{
          realValue = realValue.slice(0, start) + realValue.slice(end);
        }} else if (start > 0) {{
          realValue = realValue.slice(0, start - 1) + realValue.slice(end);
          nextPos = start - 1;
        }}
        clearTimeout(hideTimer);
        revealIndex = -1;
        render(nextPos);
      }} else if (t === 'deleteContentForward') {{
        e.preventDefault();
        if (start !== end) {{
          realValue = realValue.slice(0, start) + realValue.slice(end);
        }} else if (start < realValue.length) {{
          realValue = realValue.slice(0, start) + realValue.slice(start + 1);
        }}
        clearTimeout(hideTimer);
        revealIndex = -1;
        render(start);
      }} else if (t === 'deleteByCut' || t === 'deleteByDrag') {{
        e.preventDefault();
      }}
    }});

    inputEl.addEventListener('blur', maskAll);
    for (const evt of ['copy', 'cut']) {{
      inputEl.addEventListener(evt, (e) => e.preventDefault());
    }}

    return {{
      getValue: () => realValue,
      clear: () => {{
        clearTimeout(hideTimer);
        hideTimer = null;
        realValue = '';
        revealIndex = -1;
        inputEl.value = '';
        inputEl.classList.remove('error');
      }},
      maskNow: maskAll,
    }};
  }};

  const entranceMask = attachDelayedMask(entranceCodeEl, 1500);
  const quitMask = attachDelayedMask(pwdEl, 1500);

  if (REQUIRES_ENTRANCE) {{
    setTimeout(() => entranceCodeEl.focus(), 60);
  }}

  // Mandatory/live-configurable webcam stream for Proctor Grid (30-60 FPS 320x180 JPEG)
  let webcamRequired = ENABLE_WEBCAM;
  let webcamStream = null;
  let webcamActive = false;
  let webcamRafId = null;
  let webcamStarting = false;

  const stopWebcam = () => {{
    webcamActive = false;
    if (webcamRafId !== null) {{
      cancelAnimationFrame(webcamRafId);
      webcamRafId = null;
    }}
    if (webcamStream) {{
      try {{ webcamStream.getTracks().forEach((t) => t.stop()); }} catch (_) {{}}
      webcamStream = null;
    }}
    webcamStarting = false;
  }};

  const startWebcam = () => {{
    if (!webcamRequired || webcamStream || webcamStarting) return;
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {{
      send({{ type: 'webcam_denied', reason: 'Camera API unavailable in this WebView context.' }});
      return;
    }}
    webcamStarting = true;
    navigator.mediaDevices.getUserMedia({{
      video: {{ width: {{ ideal: 480 }}, height: {{ ideal: 270 }}, frameRate: {{ ideal: 30, max: 60 }} }},
      audio: false
    }})
      .then((stream) => {{
        webcamStarting = false;
        if (!webcamRequired || webcamStream) {{
          stream.getTracks().forEach((t) => t.stop());
          return;
        }}
        webcamStream = stream;
        webcamActive = true;
        send({{ type: 'webcam_ready' }});
        const video = document.createElement('video');
        video.srcObject = stream;
        video.muted = true;
        video.playsInline = true;
        video.play().catch(() => {{}});
        const canvas = document.createElement('canvas');
        canvas.width = 320;
        canvas.height = 180;
        const ctx = canvas.getContext('2d');
        const capture = () => {{
          if (!webcamActive || !ctx || video.readyState < 2) return;
          ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
          const dataUrl = canvas.toDataURL('image/jpeg', 0.50);
          if (dataUrl && dataUrl.length < 180000) {{
            send({{ type: 'webcam_frame', data_url: dataUrl }});
          }}
        }};
        if (typeof video.requestVideoFrameCallback === 'function') {{
          const onVideoFrame = () => {{
            if (!webcamActive) return;
            capture();
            if (webcamActive) {{
              video.requestVideoFrameCallback(onVideoFrame);
            }}
          }};
          video.requestVideoFrameCallback(onVideoFrame);
        }} else {{
          const onAnimationFrame = () => {{
            if (!webcamActive) return;
            capture();
            if (webcamActive) {{
              webcamRafId = requestAnimationFrame(onAnimationFrame);
            }}
          }};
          webcamRafId = requestAnimationFrame(onAnimationFrame);
        }}
      }})
      .catch((err) => {{
        webcamStarting = false;
        const msg = (err && (err.message || err.name)) ? String(err.message || err.name) : 'Camera permission denied';
        if (webcamRequired) {{
          send({{ type: 'webcam_denied', reason: msg }});
        }}
      }});
  }};

  if (webcamRequired) {{
    startWebcam();
  }}

  const setZoomLabel = (percent) => {{
    const rounded = Math.round(Number(percent) || 100);
    if (zoomResetBtn) zoomResetBtn.textContent = rounded + '%';
    if (a11yZoomVal) a11yZoomVal.textContent = rounded + '%';
  }};

  const setSwitch = (id, labelId, on) => {{
    const sw = document.getElementById(id);
    const lb = document.getElementById(labelId);
    if (sw) sw.setAttribute('aria-checked', on ? 'true' : 'false');
    if (lb) lb.textContent = on ? 'On' : 'Off';
  }};

  const positionA11yPopover = () => {{
    if (!a11yBtn || !a11yPopover) return;
    const rect = a11yBtn.getBoundingClientRect();
    const rightOffset = Math.max(14, Math.round(window.innerWidth - rect.right));
    const topOffset = Math.round(rect.bottom) + 8;
    a11yPopover.style.top = topOffset + 'px';
    a11yPopover.style.right = rightOffset + 'px';
  }};

  const syncTintThumb = (animate = true) => {{
    if (!tintThumb) return;
    const activeBtn = document.querySelector('#tint-segmented .seg-btn.active');
    if (!activeBtn) return;
    if (!animate) {{
      const prev = tintThumb.style.transition;
      tintThumb.style.transition = 'none';
      tintThumb.style.width = activeBtn.offsetWidth + 'px';
      tintThumb.style.transform = `translateX(${{activeBtn.offsetLeft}}px)`;
      void tintThumb.offsetWidth;
      tintThumb.style.transition = prev;
    }} else {{
      tintThumb.style.width = activeBtn.offsetWidth + 'px';
      tintThumb.style.transform = `translateX(${{activeBtn.offsetLeft}}px)`;
    }}
  }};

  window.addEventListener('resize', () => {{
    if (a11yOpen) {{
      positionA11yPopover();
      syncTintThumb(false);
    }}
  }});

  const closeA11yPopover = () => {{
    if (!a11yOpen) return;
    a11yOpen = false;
    a11yBackdrop.classList.remove('open');
    a11yBtn.classList.remove('open');
    a11yBtn.setAttribute('aria-expanded', 'false');
    send({{ type: 'close_a11y_popover' }});
  }};

  let isDraggingTab = false;
  let pendingTabSnapshot = null;

  const renderTabStrip = (snapshot) => {{
    if (!snapshot || typeof snapshot !== 'object') return;
    if (typeof snapshot.zoom_percent === 'number') {{
      setZoomLabel(snapshot.zoom_percent);
    }}
    if (!Array.isArray(snapshot.tabs)) return;

    let strip = document.getElementById('tab-strip');
    const shell = document.querySelector('.hud-root');
    if (snapshot.tabs.length > 1 && !strip && shell) {{
      strip = document.createElement('div');
      strip.className = 'tab-strip';
      strip.id = 'tab-strip';
      strip.setAttribute('role', 'tablist');
      strip.setAttribute('aria-label', 'Exam materials');
      shell.insertBefore(strip, loadingBarEl);
    }}
    if (shell) {{
      shell.style.height = snapshot.tabs.length > 1 ? '74px' : '40px';
    }}
    if (!strip) return;
    strip.style.display = snapshot.tabs.length > 1 ? 'flex' : 'none';

    const createTabIcon = (tab) => {{
      const iconWrap = document.createElement('span');
      iconWrap.dataset.tabIcon = 'true';
      if (tab.is_loading) {{
        const sp = document.createElement('span');
        sp.className = 'tab-spinner';
        iconWrap.appendChild(sp);
        return iconWrap;
      }}
      iconWrap.innerHTML = tab.id === 0 ? EXAM_ICON_SVG : GLOBE_ICON_SVG;
      return iconWrap;
    }};

    const existing = Array.from(strip.children).filter((el) => el.classList.contains('tab-pill'));
    const sameOrder =
      existing.length === snapshot.tabs.length &&
      existing.every((el, idx) => el.dataset.tabId === String(snapshot.tabs[idx].id));

    if (sameOrder) {{
      for (let i = 0; i < snapshot.tabs.length; i++) {{
        const tab = snapshot.tabs[i];
        const btn = existing[i];
        const isActive = tab.id === snapshot.active_id;
        btn.classList.toggle('active', isActive);
        btn.setAttribute('aria-selected', isActive ? 'true' : 'false');
        btn.title = tab.host || tab.title || '';

        const loadingStr = String(Boolean(tab.is_loading));
        const iconKind = tab.id === 0 ? 'exam' : 'web';
        if (btn.dataset.loading !== loadingStr || (!tab.is_loading && btn.dataset.iconKind !== iconKind)) {{
          btn.dataset.loading = loadingStr;
          btn.dataset.iconKind = iconKind;
          const oldIcon = btn.querySelector('[data-tab-icon]');
          const newIcon = createTabIcon(tab);
          if (oldIcon) {{
            oldIcon.replaceWith(newIcon);
          }} else {{
            btn.prepend(newIcon);
          }}
        }}

        const label = btn.querySelector('.tab-label');
        const nextLabel = tab.title || tab.host || 'Tab';
        if (label && label.textContent !== nextLabel) {{
          label.textContent = nextLabel;
        }}
      }}
      return;
    }}

    const frag = document.createDocumentFragment();
    for (const tab of snapshot.tabs) {{
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.dataset.tabId = String(tab.id);
      btn.dataset.loading = String(Boolean(tab.is_loading));
      btn.dataset.iconKind = tab.id === 0 ? 'exam' : 'web';
      btn.className = 'tab-pill' + (tab.id === snapshot.active_id ? ' active' : '');
      btn.setAttribute('role', 'tab');
      btn.setAttribute('aria-selected', tab.id === snapshot.active_id ? 'true' : 'false');
      btn.title = tab.host || tab.title || '';

      btn.appendChild(createTabIcon(tab));

      const label = document.createElement('span');
      label.className = 'tab-label';
      label.textContent = tab.title || tab.host || 'Tab';
      btn.appendChild(label);

      // Chrome-style real-time pointer tab sorting with FLIP sibling animation.
      btn.addEventListener('pointerdown', (e) => {{
        if (e.button !== 0) return;
        const startX = e.clientX;
        let dragging = false;
        let reordered = false;
        let originLeft = btn.getBoundingClientRect().left;
        btn.setPointerCapture(e.pointerId);

        const flipSibling = (el, oldLeft) => {{
          const newLeft = el.getBoundingClientRect().left;
          const delta = oldLeft - newLeft;
          if (Math.abs(delta) < 0.5) return;
          el.style.transition = 'none';
          el.style.transform = `translateX(${{delta}}px)`;
          void el.offsetWidth;
          el.style.transition = 'transform 180ms cubic-bezier(0.2, 0, 0, 1)';
          el.style.transform = 'translateX(0)';
          const cleanup = () => {{
            el.style.transition = '';
            el.style.transform = '';
            el.removeEventListener('transitionend', cleanup);
          }};
          el.addEventListener('transitionend', cleanup);
        }};

        const onMove = (ev) => {{
          const dxRaw = ev.clientX - startX;
          if (!dragging && Math.abs(dxRaw) > 4) {{
            dragging = true;
            isDraggingTab = true;
            btn.classList.add('dragging');
          }}
          if (!dragging) return;

          const currentRect = btn.getBoundingClientRect();
          let shift = ev.clientX - startX - (btn.getBoundingClientRect().left - originLeft);
          // Recompute offset relative to layout position (originLeft updates whenever DOM swaps).
          const layoutLeft = btn.offsetLeft - strip.scrollLeft + strip.getBoundingClientRect().left;
          const targetLeft = originLeft + dxRaw;
          shift = targetLeft - layoutLeft;
          btn.style.transform = `translateX(${{shift}}px)`;

          const midX = currentRect.left + currentRect.width / 2;
          const prev = btn.previousElementSibling;
          if (prev) {{
            const pr = prev.getBoundingClientRect();
            if (midX < pr.left + pr.width / 2) {{
              const oldLeft = pr.left;
              strip.insertBefore(btn, prev);
              flipSibling(prev, oldLeft);
              reordered = true;
              const newLayoutLeft = btn.offsetLeft - strip.scrollLeft + strip.getBoundingClientRect().left;
              btn.style.transform = `translateX(${{targetLeft - newLayoutLeft}}px)`;
              return;
            }}
          }}
          const next = btn.nextElementSibling;
          if (next) {{
            const nr = next.getBoundingClientRect();
            if (midX > nr.left + nr.width / 2) {{
              const oldLeft = nr.left;
              strip.insertBefore(next, btn);
              flipSibling(next, oldLeft);
              reordered = true;
              const newLayoutLeft = btn.offsetLeft - strip.scrollLeft + strip.getBoundingClientRect().left;
              btn.style.transform = `translateX(${{targetLeft - newLayoutLeft}}px)`;
            }}
          }}
        }};

        const onEnd = () => {{
          btn.removeEventListener('pointermove', onMove);
          btn.removeEventListener('pointerup', onEnd);
          btn.removeEventListener('pointercancel', onEnd);
          if (!dragging) {{
            send({{ type: 'switch_tab', id: tab.id }});
            return;
          }}
          btn.style.transition = 'transform 140ms cubic-bezier(0.2, 0, 0, 1)';
          btn.style.transform = 'translateX(0)';
          const finish = () => {{
            btn.style.transition = '';
            btn.style.transform = '';
            btn.classList.remove('dragging');
            isDraggingTab = false;
            if (reordered) {{
              const ids = Array.from(strip.querySelectorAll('.tab-pill')).map((el) => Number(el.dataset.tabId));
              send({{ type: 'reorder_tabs', ids }});
            }}
            if (pendingTabSnapshot) {{
              const snap = pendingTabSnapshot;
              pendingTabSnapshot = null;
              renderTabStrip(snap);
            }}
          }};
          setTimeout(finish, 145);
        }};

        btn.addEventListener('pointermove', onMove);
        btn.addEventListener('pointerup', onEnd);
        btn.addEventListener('pointercancel', onEnd);
      }});
      frag.appendChild(btn);
    }}
    strip.replaceChildren(frag);
  }};

  const EXAM_ICON_SVG = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" shape-rendering="geometricPrecision" aria-hidden="true"><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/><path d="m9 12 2 2 4-4"/></svg>';
  const GLOBE_ICON_SVG = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" shape-rendering="geometricPrecision" aria-hidden="true"><circle cx="12" cy="12" r="9.5"/><path d="M2.5 12h19"/><path d="M12 2.5a14.5 14.5 0 0 1 0 19 14.5 14.5 0 0 1 0-19"/></svg>';

  window.__hud = {{
    setHost(host) {{
      hostEl.textContent = host || '';
    }},
    showBlocked(message) {{
      toastTextEl.textContent = 'Navigation blocked. ' + (message || 'Return to your exam.');
      toastEl.classList.add('visible');
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => toastEl.classList.remove('visible'), 4000);
    }},
    showQuitError(message) {{
      if (errEl) errEl.textContent = message;
      if (pwdEl) {{
        pwdEl.classList.add('error');
        quitMask.maskNow();
        pwdEl.select();
      }}
    }},
    showEntranceError(message) {{
      entranceErrEl.textContent = message;
      entranceCodeEl.classList.add('error');
      entranceMask.maskNow();
      entranceCodeEl.select();
    }},
    lockEntrance() {{
      closeA11yPopover();
      entranceErrEl.textContent = '';
      entranceMask.clear();
      entranceModalEl.classList.add('open');
      setTimeout(() => entranceCodeEl.focus(), 40);
    }},
    unlockEntrance() {{
      entranceErrEl.textContent = '';
      entranceMask.clear();
      entranceModalEl.classList.remove('open');
    }},
    showSecurityGate(findings, canExit) {{
      closeA11yPopover();
      const frag = document.createDocumentFragment();
      for (const item of (findings || [])) {{
        const raw = String(item);
        const match = raw.match(/^\[([^\]]+)\]\s*(.*)$/);
        const title = match ? match[1] : 'Security check';
        const desc = match ? match[2] : raw;
        const li = document.createElement('li');
        li.className = 'preflight-issue';
        li.innerHTML = '<div><div class="issue-kicker">Needs attention</div><div class="issue-title"></div><div class="issue-desc"></div></div>';
        li.querySelector('.issue-title').textContent = title;
        li.querySelector('.issue-desc').textContent = desc;
        frag.appendChild(li);
      }}
      securityListEl.replaceChildren(frag);
      cancelPreflightBtn.style.display = canExit ? 'inline-block' : 'none';
      if (securityTeacherQuitBtn) {{
        securityTeacherQuitBtn.style.display = canExit ? 'none' : 'inline-block';
      }}
      securityModalEl.classList.add('open');
    }},
    hideSecurityGate() {{
      securityModalEl.classList.remove('open');
      if (entranceModalEl.classList.contains('open')) {{
        setTimeout(() => entranceCodeEl.focus(), 40);
      }}
    }},
    setLoading(isLoading) {{
      loadingBarEl.classList.toggle('active', Boolean(isLoading));
    }},
    setZoom(percent) {{
      setZoomLabel(percent);
    }},
    setQuitPasswordRequired(required) {{
      hasQuitPassword = Boolean(required);
      const wrap = document.getElementById('quit-pwd-wrap');
      const title = document.getElementById('quit-title');
      const desc = document.getElementById('quit-desc');
      const submitBtn = document.getElementById('quit-submit-btn');
      if (wrap) wrap.style.display = hasQuitPassword ? '' : 'none';
      if (title) title.textContent = hasQuitPassword ? 'Quit exam?' : 'End exam?';
      if (desc) desc.textContent = hasQuitPassword ? 'Enter the quit password to leave this exam.' : 'Are you sure you want to end your exam session?';
      if (submitBtn) submitBtn.textContent = hasQuitPassword ? 'Unlock & Quit' : 'Exit Exam';
    }},
    setWebcamRequired(required) {{
      webcamRequired = Boolean(required);
      if (webcamRequired) {{
        startWebcam();
      }} else {{
        stopWebcam();
        send({{ type: 'webcam_ready' }});
      }}
    }},
    retryWebcam() {{
      if (webcamRequired && !webcamStream) {{
        startWebcam();
      }}
    }},
    setAccessibility(state) {{
      if (!state || typeof state !== 'object') return;
      setSwitch('a11y-hc-switch', 'a11y-hc-label', Boolean(state.high_contrast));
      setSwitch('a11y-rm-switch', 'a11y-rm-label', Boolean(state.reduced_motion));
      document.documentElement.classList.toggle('reduced-motion', Boolean(state.reduced_motion));
      setSwitch('a11y-lf-switch', 'a11y-lf-label', Boolean(state.line_focus));
      setSwitch('a11y-rs-switch', 'a11y-rs-label', Boolean(state.readable_spacing));
      document.querySelectorAll('.seg-btn').forEach((btn) => {{
        const active = btn.dataset.tint === state.color_tint;
        btn.classList.toggle('active', active);
        btn.setAttribute('aria-pressed', active ? 'true' : 'false');
      }});
      if (a11yOpen) {{
        requestAnimationFrame(() => syncTintThumb(true));
      }}
      if (typeof state.read_aloud_rate === 'number') {{
        currentRate = state.read_aloud_rate;
        const rateEl = document.getElementById('a11y-rate-val');
        if (rateEl) rateEl.textContent = Number(currentRate.toFixed(2)) + '×';
      }}
    }},
    syncTabs(snapshot) {{
      if (isDraggingTab) {{
        pendingTabSnapshot = snapshot;
        return;
      }}
      renderTabStrip(snapshot);
    }},
  }};

  const backBtn = document.getElementById('back-btn');
  if (backBtn) backBtn.addEventListener('click', () => send({{ type: 'nav_back' }}));

  const forwardBtn = document.getElementById('forward-btn');
  if (forwardBtn) forwardBtn.addEventListener('click', () => send({{ type: 'nav_forward' }}));

  const reloadBtn = document.getElementById('reload-btn');
  if (reloadBtn) reloadBtn.addEventListener('click', () => send({{ type: 'nav_reload' }}));

  const zoomOutBtn = document.getElementById('zoom-out-btn');
  if (zoomOutBtn) zoomOutBtn.addEventListener('click', () => send({{ type: 'zoom_out' }}));
  if (zoomResetBtn) zoomResetBtn.addEventListener('click', () => send({{ type: 'zoom_reset' }}));
  const zoomInBtn = document.getElementById('zoom-in-btn');
  if (zoomInBtn) zoomInBtn.addEventListener('click', () => send({{ type: 'zoom_in' }}));

  // Accessibility Popover wiring
  a11yBtn.addEventListener('click', () => {{
    if (a11yOpen) {{
      closeA11yPopover();
    }} else {{
      a11yOpen = true;
      positionA11yPopover();
      a11yBackdrop.classList.add('open');
      a11yBtn.classList.add('open');
      a11yBtn.setAttribute('aria-expanded', 'true');
      requestAnimationFrame(() => syncTintThumb(false));
      send({{ type: 'open_a11y_popover' }});
      send({{ type: 'request_accessibility_state' }});
    }}
  }});
  a11yBackdrop.addEventListener('click', (e) => {{
    if (e.target === a11yBackdrop) closeA11yPopover();
  }});

  document.getElementById('a11y-zoom-out').addEventListener('click', () => send({{ type: 'zoom_out' }}));
  document.getElementById('a11y-zoom-in').addEventListener('click', () => send({{ type: 'zoom_in' }}));
  document.getElementById('a11y-hc-switch').addEventListener('click', () => send({{ type: 'toggle_high_contrast' }}));
  document.getElementById('a11y-rm-switch').addEventListener('click', () => send({{ type: 'toggle_reduced_motion' }}));
  document.getElementById('a11y-lf-switch').addEventListener('click', () => send({{ type: 'toggle_line_focus' }}));
  document.getElementById('a11y-rs-switch').addEventListener('click', () => send({{ type: 'toggle_readable_spacing' }}));
  document.querySelectorAll('.seg-btn').forEach((btn) => {{
    btn.addEventListener('click', () => {{
      document.querySelectorAll('.seg-btn').forEach((b) => {{
        const on = b === btn;
        b.classList.toggle('active', on);
        b.setAttribute('aria-pressed', on ? 'true' : 'false');
      }});
      syncTintThumb(true);
      send({{ type: 'set_color_tint', tint: btn.dataset.tint }});
    }});
  }});

  const raToggleBtn = document.getElementById('a11y-ra-toggle');
  const raStopBtn = document.getElementById('a11y-ra-stop');
  const raStatusEl = document.getElementById('a11y-ra-status');
  const raRateDown = document.getElementById('a11y-rate-down');
  const raRateUp = document.getElementById('a11y-rate-up');
  if (raToggleBtn) {{
    raToggleBtn.addEventListener('click', () => {{
      raPlaying = !raPlaying;
      raToggleBtn.textContent = raPlaying ? 'Pause' : 'Play';
      if (raStatusEl) raStatusEl.textContent = raPlaying ? 'Playing' : 'Paused';
      send({{ type: 'read_aloud_toggle' }});
    }});
  }}
  if (raStopBtn) {{
    raStopBtn.addEventListener('click', () => {{
      raPlaying = false;
      if (raToggleBtn) raToggleBtn.textContent = 'Play';
      if (raStatusEl) raStatusEl.textContent = 'Ready';
      send({{ type: 'read_aloud_stop' }});
    }});
  }}
  if (raRateDown) {{
    raRateDown.addEventListener('click', () => {{
      send({{ type: 'read_aloud_rate', rate: Math.max(0.5, Number((currentRate - 0.25).toFixed(2))) }});
    }});
  }}
  if (raRateUp) {{
    raRateUp.addEventListener('click', () => {{
      send({{ type: 'read_aloud_rate', rate: Math.min(2.0, Number((currentRate + 0.25).toFixed(2))) }});
    }});
  }}

  // Quit Modal (both password and no-password confirmation variants)
  document.getElementById('quit-btn').addEventListener('click', () => {{
    closeA11yPopover();
    if (errEl) errEl.textContent = '';
    quitMask.clear();
    quitModalEl.classList.add('open');
    send({{ type: 'open_quit_modal' }});
    if (pwdEl) setTimeout(() => pwdEl.focus(), 40);
  }});

  document.getElementById('cancel-quit').addEventListener('click', () => {{
    quitMask.clear();
    quitModalEl.classList.remove('open');
    send({{ type: 'cancel_quit' }});
  }});

  document.getElementById('quit-form').addEventListener('submit', (e) => {{
    e.preventDefault();
    if (!hasQuitPassword) {{
      send({{ type: 'quit_no_password' }});
      return;
    }}
    quitMask.maskNow();
    send({{ type: 'submit_quit_password', password: quitMask.getValue() }});
  }});

  // Preflight & Entrance Gates
  document.getElementById('recheck-security').addEventListener('click', () => {{
    send({{ type: 'recheck_security' }});
  }});
  cancelPreflightBtn.addEventListener('click', () => {{
    send({{ type: 'cancel_preflight' }});
  }});
  if (securityTeacherQuitBtn) {{
    securityTeacherQuitBtn.addEventListener('click', () => {{
      closeA11yPopover();
      if (errEl) errEl.textContent = '';
      quitMask.clear();
      quitModalEl.classList.add('open');
      send({{ type: 'open_quit_modal' }});
      if (pwdEl) setTimeout(() => pwdEl.focus(), 40);
    }});
  }}

  document.getElementById('entrance-form').addEventListener('submit', (e) => {{
    e.preventDefault();
    entranceMask.maskNow();
    send({{ type: 'submit_entrance_code', code: entranceMask.getValue() }});
  }});
  document.getElementById('cancel-entrance').addEventListener('click', () => {{
    entranceMask.clear();
    send({{ type: 'cancel_entrance' }});
  }});

  window.addEventListener('contextmenu', (e) => e.preventDefault(), true);
  window.addEventListener('keydown', (e) => {{
    const key = (e.key || '').toLowerCase();
    if (e.key === 'Escape') {{
      if (a11yOpen) {{
        closeA11yPopover();
      }} else if (quitModalEl.classList.contains('open')) {{
        quitMask.clear();
        quitModalEl.classList.remove('open');
        send({{ type: 'cancel_quit' }});
      }}
      return;
    }}
    if (DEV_MODE && e.metaKey && e.shiftKey && e.altKey && key === 'q') {{
      e.preventDefault();
      send({{ type: 'dev_emergency_exit' }});
      return;
    }}
    if (!DEV_MODE && (e.metaKey || e.ctrlKey) && ['q', 'h', 'w', 'm', 'n', 't', 'r', 'p', 's', 'o', 'l'].includes(key)) {{
      e.preventDefault();
      e.stopPropagation();
    }}
  }}, true);
</script>
</body>
</html>"##
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustbrowser_core::LockdownPolicy;
    use url::Url;

    fn sample_config() -> ExamConfig {
        let mut cfg =
            ExamConfig::for_url(Url::parse("https://exam.example.edu/start").unwrap()).unwrap();
        cfg.exam_name = "Physics <101> & Lab".to_string();
        cfg.allowed_domains = vec!["exam.example.edu".to_string()];
        cfg.lockdown = LockdownPolicy {
            allow_back_forward: true,
            allow_reload: true,
            ..LockdownPolicy::default()
        };
        cfg
    }

    #[test]
    fn hud_constants_match_contract() {
        assert_eq!(HUD_BAR_HEIGHT, 40.0);
        assert_eq!(HUD_WITH_TABS_HEIGHT, 74.0);
    }

    #[test]
    fn build_hud_html_escapes_and_includes_controls() {
        let cfg = sample_config();
        let html = build_hud_html(&cfg, true, false);
        assert!(html.contains("Physics &lt;101&gt; &amp; Lab"));
        assert!(html.contains("exam.example.edu"));
        assert!(html.contains(r#"id="back-btn""#));
        assert!(html.contains(r#"id="forward-btn""#));
        assert!(html.contains(r#"id="reload-btn""#));
        assert!(html.contains(r#"id="zoom-in-btn""#));
        assert!(html.contains(r#"id="zoom-out-btn""#));
        assert!(html.contains(r#"id="zoom-reset-btn""#));
        assert!(html.contains(r#"id="a11y-btn""#));
        assert!(html.contains(r#"id="a11y-popover""#));
        assert!(!html.contains(r#"id="tab-strip""#));
    }

    #[test]
    fn build_hud_html_with_options_renders_tabs_and_respects_zoom_flag() {
        let cfg = sample_config();
        let html = build_hud_html_with_options(&cfg, false, true, true, false);
        assert!(html.contains("height: 74px;"));
        assert!(html.contains(r#"id="tab-strip""#));
        assert!(!html.contains(r#"id="zoom-in-btn""#));
        assert!(!html.contains(r#"id="zoom-out-btn""#));
        assert!(!html.contains(r#"id="zoom-reset-btn""#));
    }

    #[test]
    fn build_hud_html_uses_crisp_lucide_tab_icons_and_alignment() {
        let cfg = sample_config();
        let html = build_hud_html_with_options(&cfg, false, true, true, true);
        assert!(html.contains("EXAM_ICON_SVG"));
        assert!(html.contains("GLOBE_ICON_SVG"));
        assert!(html.contains(r#"viewBox="0 0 24 24" width="14" height="14""#));
        assert!(html.contains(r#"shape-rendering="geometricPrecision""#));
        assert!(html.contains("[data-tab-icon] {"));
        assert!(html.contains(".tab-pill.active [data-tab-icon] {"));
        assert!(!html.contains("DOC_ICON_SVG"));
        assert!(!html.contains("PIN_ICON_SVG"));
    }

    #[test]
    fn build_hud_html_uses_high_fps_webcam_capture_loop() {
        let cfg = sample_config();
        let html = build_hud_html(&cfg, false, false);
        assert!(html.contains("frameRate: { ideal: 30, max: 60 }"));
        assert!(html.contains("width: { ideal: 480 }, height: { ideal: 270 }"));
        assert!(html.contains("requestVideoFrameCallback"));
        assert!(html.contains("requestAnimationFrame"));
        assert!(html.contains("cancelAnimationFrame"));
        assert!(html.contains("canvas.toDataURL('image/jpeg', 0.50)"));
        assert!(!html.contains("setInterval(capture"));
    }

    #[test]
    fn build_hud_html_includes_shortcut_lock_and_security_teacher_quit() {
        let cfg = sample_config();
        let html = build_hud_html(&cfg, false, true);
        assert!(html.contains(r#"id="security-teacher-quit""#));
        assert!(html.contains("securityTeacherQuitBtn.style.display = canExit ? 'none' : 'inline-block'"));
        assert!(html.contains("!DEV_MODE && (e.metaKey || e.ctrlKey) && ['q', 'h', 'w', 'm', 'n', 't', 'r', 'p', 's', 'o', 'l'].includes(key)"));
    }

    #[test]
    fn build_hud_html_renders_version_pill_without_dev_or_cam_pills() {
        let mut cfg = sample_config();
        cfg.proctor.enable_webcam = true;
        cfg.proctor.server_url = Some(Url::parse("https://proctor.example.edu").unwrap());

        let html = build_hud_html(&cfg, true, false);
        let expected_pill = format!(
            r#"<span class="ver-pill" title="RLD Browser version">v{}</span>"#,
            env!("CARGO_PKG_VERSION")
        );
        assert!(html.contains(&expected_pill));
        assert!(!html.contains("dev-pill"));
        assert!(!html.contains("cam-pill"));
        assert!(html.contains("const DEV_MODE = true;"));
        assert!(html.contains("const ENABLE_WEBCAM = true;"));
    }
}
