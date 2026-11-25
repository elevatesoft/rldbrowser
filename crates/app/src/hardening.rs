use rustbrowser_core::ColorTint;
#[cfg(test)]
use rustbrowser_core::LockdownPolicy;

/// Options controlling document-start JavaScript hardening in the exam WebView.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardeningOptions {
    pub block_new_windows: bool,
    pub block_clipboard: bool,
    pub block_context_menu: bool,
    pub block_devtools: bool,
    pub allow_reload: bool,
    pub allow_zoom: bool,
    pub allow_find_in_page: bool,
    pub allow_read_aloud: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    pub color_tint: ColorTint,
    pub line_focus: bool,
    pub readable_spacing: bool,
    pub dev_mode: bool,
}

/// Generates a document-start JavaScript hardening script tailored to the active `LockdownPolicy`.
/// Runs before any page script executes in the exam WebView.
#[cfg(test)]
pub fn build_init_script(policy: &LockdownPolicy, dev_mode: bool) -> String {
    let opts = HardeningOptions {
        block_new_windows: policy.block_new_windows,
        block_clipboard: policy.block_clipboard,
        block_context_menu: policy.block_context_menu,
        block_devtools: policy.block_devtools,
        allow_reload: policy.allow_reload,
        allow_zoom: false,
        allow_find_in_page: false,
        allow_read_aloud: false,
        high_contrast: false,
        reduced_motion: false,
        color_tint: ColorTint::Default,
        line_focus: false,
        readable_spacing: false,
        dev_mode,
    };
    build_init_script_with_options(&opts)
}

/// Generates a document-start JavaScript hardening script from explicit `HardeningOptions`.
/// Runs before any page script executes in the exam WebView.
pub fn build_init_script_with_options(opts: &HardeningOptions) -> String {
    format!(
        r#"
(() => {{
  const POLICY = {{
    blockNewWindows: {block_new_windows},
    blockClipboard: {block_clipboard},
    blockContextMenu: {block_context_menu},
    blockDevtools: {block_devtools},
    allowReload: {allow_reload},
    allowZoom: {allow_zoom},
    allowFindInPage: {allow_find_in_page},
    allowReadAloud: {allow_read_aloud},
    highContrast: {high_contrast},
    reducedMotion: {reduced_motion},
    colorTint: '{color_tint}',
    lineFocus: {line_focus},
    readableSpacing: {readable_spacing},
    devMode: {dev_mode},
  }};

  const postIpc = window.ipc && window.ipc.postMessage ? window.ipc.postMessage.bind(window.ipc) : null;
  const safeStringify = JSON.stringify;

  if (POLICY.blockNewWindows) {{
    try {{
      Object.defineProperty(window, 'open', {{
        value: () => null,
        writable: false,
        configurable: false,
      }});
    }} catch (_) {{}}
  }}

  // Always block window.print in an exam environment
  try {{
    Object.defineProperty(window, 'print', {{
      value: () => {{}},
      writable: false,
      configurable: false,
    }});
  }} catch (_) {{}}

  if (POLICY.blockContextMenu) {{
    window.addEventListener('contextmenu', (e) => {{
      e.preventDefault();
      e.stopPropagation();
    }}, true);
  }}

  if (POLICY.blockClipboard) {{
    for (const evt of ['copy', 'cut', 'paste', 'drop', 'dragstart']) {{
      window.addEventListener(evt, (e) => {{
        e.preventDefault();
        e.stopPropagation();
      }}, true);
    }}
    try {{
      const blockedClipboard = Object.freeze({{
        readText: () => Promise.reject(new DOMException('Clipboard blocked', 'NotAllowedError')),
        writeText: () => Promise.reject(new DOMException('Clipboard blocked', 'NotAllowedError')),
        read: () => Promise.reject(new DOMException('Clipboard blocked', 'NotAllowedError')),
        write: () => Promise.reject(new DOMException('Clipboard blocked', 'NotAllowedError')),
      }});
      if (navigator.clipboard) {{
        for (const fn of ['readText', 'writeText', 'read', 'write']) {{
          try {{
            Object.defineProperty(navigator.clipboard, fn, {{
              value: blockedClipboard[fn],
              writable: false,
              configurable: false,
            }});
          }} catch (_) {{}}
        }}
      }}
      try {{
        Object.defineProperty(navigator, 'clipboard', {{
          value: blockedClipboard,
          writable: false,
          configurable: false,
        }});
      }} catch (_) {{}}
    }} catch (_) {{}}
  }}

  window.addEventListener('keydown', (e) => {{
    const meta = e.metaKey || e.ctrlKey;
    const key = (e.key || '').toLowerCase();

    // Emergency escape in --dev mode: Cmd+Shift+Alt+Q
    if (POLICY.devMode && e.metaKey && e.shiftKey && e.altKey && key === 'q') {{
      e.preventDefault();
      postIpc && postIpc(safeStringify({{ type: 'dev_emergency_exit' }}));
      return;
    }}

    // Block DevTools shortcuts (F12, Cmd+Opt+I/J/C/U)
    if (POLICY.blockDevtools) {{
      if (e.key === 'F12' || (meta && e.altKey && ['i', 'j', 'c', 'k'].includes(key)) || (meta && key === 'u')) {{
        e.preventDefault();
        e.stopPropagation();
        return;
      }}
    }}

    // Intercept zoom shortcuts: prevent native browser zoom and forward to host if allowed
    if (meta && (key === '=' || key === '+')) {{
      e.preventDefault();
      e.stopPropagation();
      if (POLICY.allowZoom) {{
        postIpc && postIpc(safeStringify({{ type: 'zoom_in' }}));
      }}
      return;
    }}
    if (meta && (key === '-' || key === '_')) {{
      e.preventDefault();
      e.stopPropagation();
      if (POLICY.allowZoom) {{
        postIpc && postIpc(safeStringify({{ type: 'zoom_out' }}));
      }}
      return;
    }}
    if (meta && key === '0') {{
      e.preventDefault();
      e.stopPropagation();
      if (POLICY.allowZoom) {{
        postIpc && postIpc(safeStringify({{ type: 'zoom_reset' }}));
      }}
      return;
    }}

    // Block find-in-page shortcut if disallowed
    if (!POLICY.allowFindInPage && meta && key === 'f') {{
      e.preventDefault();
      e.stopPropagation();
      return;
    }}

    // Block clipboard shortcuts
    if (POLICY.blockClipboard && meta && ['c', 'v', 'x'].includes(key)) {{
      e.preventDefault();
      e.stopPropagation();
      return;
    }}

    // Block new tab / new window / close / print / save / location bar / quit
    if (meta && ['t', 'n', 'w', 'p', 's', 'l', 'o', 'q', 'h', 'm'].includes(key)) {{
      e.preventDefault();
      e.stopPropagation();
      return;
    }}

    if (!POLICY.allowReload && ((meta && key === 'r') || e.key === 'F5')) {{
      e.preventDefault();
      e.stopPropagation();
      return;
    }}
  }}, true);

  let lastTitle = '';
  const reportTitle = () => {{
    const title = document.title || '';
    if (title && title !== lastTitle) {{
      lastTitle = title;
      postIpc && postIpc(safeStringify({{ type: 'title_changed', title }}));
    }}
  }};

  const observeTitle = () => {{
    reportTitle();
    const target = document.head || document.documentElement;
    if (!target || typeof MutationObserver === 'undefined') return;
    const observer = new MutationObserver(() => {{
      reportTitle();
    }});
    observer.observe(target, {{ childList: true, subtree: true, characterData: true }});
  }};

  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', observeTitle, {{ once: true }});
  }} else {{
    observeTitle();
  }}

  // Accessibility runtime controller (Read-Aloud + Voice, High Contrast, Reduced Motion, Color Tint, Line Focus, Readable Spacing)
  const A11Y_STYLE_ID = '__rustbrowser_a11y_style';
  const WARM_OVERLAY_ID = '__rustbrowser_warm_overlay';
  const LINE_FOCUS_ID = '__rustbrowser_line_focus';
  const LINE_FOCUS_BAND_PX = 84;
  let a11yState = {{
    highContrast: POLICY.highContrast,
    reducedMotion: POLICY.reducedMotion,
    colorTint: POLICY.colorTint,
    lineFocus: POLICY.lineFocus,
    readableSpacing: POLICY.readableSpacing,
    modalBlur: false,
    rate: 1.0,
    voiceName: null,
    lineFocusY: 220,
  }};

  const renderWarmOverlay = () => {{
    let overlay = document.getElementById(WARM_OVERLAY_ID);
    if (a11yState.colorTint !== 'warm') {{
      if (overlay) overlay.remove();
      return;
    }}
    if (!document.body) return;
    if (!overlay) {{
      overlay = document.createElement('div');
      overlay.id = WARM_OVERLAY_ID;
      overlay.setAttribute('aria-hidden', 'true');
      overlay.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483645;background:#FBF5E6;mix-blend-mode:multiply;';
      document.body.appendChild(overlay);
    }}
  }};

  const applyA11yStyles = () => {{
    const root = document.head || document.documentElement;
    if (!root) return;
    let el = document.getElementById(A11Y_STYLE_ID);
    const filters = [];
    const rules = [];

    if (a11yState.colorTint === 'warm') {{
      rules.push('html, body {{ background-color: #FBF5E6 !important; color: #26211A !important; }}');
    }} else if (a11yState.colorTint === 'dark') {{
      rules.push('html {{ color-scheme: dark !important; background-color: #141A22 !important; }}');
      rules.push('body {{ background-color: #141A22 !important; color: #CDD6E2 !important; }}');
      rules.push('main, article, section, div, header, footer, nav, aside, table, tr, td, th, ul, ol, li, pre, code, blockquote {{ background-color: transparent !important; border-color: #2D3748 !important; color: inherit !important; }}');
      rules.push('h1, h2, h3, h4, h5, h6, strong, b {{ color: #F3F6FA !important; }}');
      rules.push('a, a:visited {{ color: #7AB3FF !important; }}');
      rules.push('input, textarea, select, button {{ background-color: #1C2430 !important; color: #F3F6FA !important; border-color: #3B495E !important; }}');
      rules.push('hr {{ border-color: #2D3748 !important; }}');
    }} else if (a11yState.colorTint === 'inverted') {{
      filters.push('invert(1) hue-rotate(180deg)');
    }}

    if (a11yState.highContrast) {{
      filters.push('contrast(1.22)');
      rules.push('a:focus, button:focus, input:focus, textarea:focus, select:focus {{ outline: 3px solid #235DBC !important; outline-offset: 2px !important; }}');
    }}

    if (a11yState.modalBlur) {{
      filters.push('blur(5px)');
    }}

    if (filters.length > 0) {{
      rules.unshift(`html {{ filter: ${{filters.join(' ')}} !important; }}`);
    }}

    if (a11yState.reducedMotion) {{
      rules.push('*, *::before, *::after {{ animation-duration: 0.001ms !important; animation-iteration-count: 1 !important; transition-duration: 0.001ms !important; scroll-behavior: auto !important; }}');
    }}

    if (a11yState.readableSpacing) {{
      rules.push('p, li, td, th, label, blockquote, article, section {{ line-height: 1.65 !important; letter-spacing: 0.05em !important; word-spacing: 0.14em !important; }}');
    }}

    renderWarmOverlay();

    if (rules.length === 0) {{
      if (el) el.remove();
      return;
    }}
    if (!el) {{
      el = document.createElement('style');
      el.id = A11Y_STYLE_ID;
      root.appendChild(el);
    }}
    el.textContent = rules.join('\n');
  }};

  const renderLineFocus = () => {{
    let mask = document.getElementById(LINE_FOCUS_ID);
    if (!a11yState.lineFocus) {{
      if (mask) mask.remove();
      return;
    }}
    if (!document.body) return;
    if (!mask) {{
      mask = document.createElement('div');
      mask.id = LINE_FOCUS_ID;
      mask.setAttribute('aria-hidden', 'true');
      mask.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483646;';
      const topShade = document.createElement('div');
      topShade.dataset.role = 'top';
      topShade.style.cssText = 'position:fixed;left:0;right:0;top:0;background:rgba(15,23,42,0.45);';
      const bottomShade = document.createElement('div');
      bottomShade.dataset.role = 'bottom';
      bottomShade.style.cssText = 'position:fixed;left:0;right:0;bottom:0;background:rgba(15,23,42,0.45);';
      mask.appendChild(topShade);
      mask.appendChild(bottomShade);
      document.body.appendChild(mask);
    }}
    const half = LINE_FOCUS_BAND_PX / 2;
    const topHeight = Math.max(0, a11yState.lineFocusY - half);
    const bottomTop = Math.max(topHeight + LINE_FOCUS_BAND_PX, a11yState.lineFocusY + half);
    const topShade = mask.querySelector('[data-role="top"]');
    const bottomShade = mask.querySelector('[data-role="bottom"]');
    if (topShade) topShade.style.height = topHeight + 'px';
    if (bottomShade) bottomShade.style.top = bottomTop + 'px';
  }};

  window.addEventListener('mousemove', (e) => {{
    if (!a11yState.lineFocus) return;
    a11yState.lineFocusY = e.clientY;
    renderLineFocus();
  }}, {{ passive: true }});

  window.addEventListener('focusin', (e) => {{
    if (!a11yState.lineFocus || !e.target || !e.target.getBoundingClientRect) return;
    const rect = e.target.getBoundingClientRect();
    if (rect.height > 0) {{
      a11yState.lineFocusY = rect.top + rect.height / 2;
      renderLineFocus();
    }}
  }}, true);

  const reportVoices = () => {{
    if (!POLICY.allowReadAloud || typeof window.speechSynthesis === 'undefined') return;
    const names = Array.from(new Set(
      (window.speechSynthesis.getVoices() || [])
        .map((v) => v && v.name)
        .filter(Boolean)
    )).slice(0, 40);
    if (names.length > 0 && postIpc) {{
      postIpc(safeStringify({{ type: 'voices_loaded', voices: names }}));
    }}
  }};

  if (POLICY.allowReadAloud && typeof window.speechSynthesis !== 'undefined') {{
    window.speechSynthesis.addEventListener &&
      window.speechSynthesis.addEventListener('voiceschanged', reportVoices);
  }}

  try {{
    Object.defineProperty(window, '__examA11y', {{
      value: Object.freeze({{
        setHighContrast(enabled) {{
          a11yState.highContrast = Boolean(enabled);
          applyA11yStyles();
        }},
        setReducedMotion(enabled) {{
          a11yState.reducedMotion = Boolean(enabled);
          applyA11yStyles();
        }},
        setColorTint(tint) {{
          if (['default', 'warm', 'dark', 'inverted'].includes(tint)) {{
            a11yState.colorTint = tint;
            applyA11yStyles();
          }}
        }},
        setLineFocus(enabled) {{
          a11yState.lineFocus = Boolean(enabled);
          renderLineFocus();
        }},
        setReadableSpacing(enabled) {{
          a11yState.readableSpacing = Boolean(enabled);
          applyA11yStyles();
        }},
        setModalBlur(enabled) {{
          a11yState.modalBlur = Boolean(enabled);
          applyA11yStyles();
        }},
        setRate(rate) {{
          const n = Number(rate);
          if (Number.isFinite(n)) a11yState.rate = Math.min(2.0, Math.max(0.5, n));
        }},
        setVoice(name) {{
          a11yState.voiceName = typeof name === 'string' && name.trim() ? name.trim() : null;
        }},
        toggleReadAloud(rate, voiceName) {{
          if (!POLICY.allowReadAloud || typeof window.speechSynthesis === 'undefined') return;
          if (typeof rate === 'number') this.setRate(rate);
          if (voiceName !== undefined) this.setVoice(voiceName);
          const synth = window.speechSynthesis;
          if (synth.speaking && !synth.paused) {{
            synth.pause();
            return;
          }}
          if (synth.paused) {{
            synth.resume();
            return;
          }}
          const selected = (window.getSelection && String(window.getSelection()).trim()) || '';
          const text = selected || (document.body && (document.body.innerText || '').trim()) || '';
          if (!text) return;
          synth.cancel();
          const utter = new SpeechSynthesisUtterance(text.slice(0, 12000));
          utter.rate = a11yState.rate;
          if (a11yState.voiceName) {{
            const match = (synth.getVoices() || []).find((v) => v && v.name === a11yState.voiceName);
            if (match) utter.voice = match;
          }}
          synth.speak(utter);
        }},
        stopReadAloud() {{
          if (typeof window.speechSynthesis !== 'undefined') {{
            window.speechSynthesis.cancel();
          }}
        }},
      }}),
      writable: false,
      configurable: false,
    }});
  }} catch (_) {{}}

  const initA11y = () => {{
    applyA11yStyles();
    renderLineFocus();
    reportVoices();
  }};

  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', initA11y, {{ once: true }});
  }} else {{
    initA11y();
  }}
}})();
"#,
        block_new_windows = opts.block_new_windows,
        block_clipboard = opts.block_clipboard,
        block_context_menu = opts.block_context_menu,
        block_devtools = opts.block_devtools,
        allow_reload = opts.allow_reload,
        allow_zoom = opts.allow_zoom,
        allow_find_in_page = opts.allow_find_in_page,
        allow_read_aloud = opts.allow_read_aloud,
        high_contrast = opts.high_contrast,
        reduced_motion = opts.reduced_motion,
        color_tint = opts.color_tint.as_str(),
        line_focus = opts.line_focus,
        readable_spacing = opts.readable_spacing,
        dev_mode = opts.dev_mode,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_script_with_options_embeds_all_policy_flags() {
        let opts = HardeningOptions {
            block_new_windows: true,
            block_clipboard: false,
            block_context_menu: true,
            block_devtools: true,
            allow_reload: false,
            allow_zoom: true,
            allow_find_in_page: true,
            allow_read_aloud: true,
            high_contrast: true,
            reduced_motion: false,
            color_tint: ColorTint::Warm,
            line_focus: true,
            readable_spacing: true,
            dev_mode: true,
        };
        let js = build_init_script_with_options(&opts);

        assert!(js.contains("blockNewWindows: true,"));
        assert!(js.contains("blockClipboard: false,"));
        assert!(js.contains("blockContextMenu: true,"));
        assert!(js.contains("blockDevtools: true,"));
        assert!(js.contains("allowReload: false,"));
        assert!(js.contains("allowZoom: true,"));
        assert!(js.contains("allowFindInPage: true,"));
        assert!(js.contains("devMode: true,"));
        assert!(js.contains("type: 'zoom_in'"));
        assert!(js.contains("type: 'zoom_out'"));
        assert!(js.contains("type: 'zoom_reset'"));
        assert!(js.contains("type: 'title_changed'"));
        assert!(js.contains("MutationObserver"));
    }

    #[test]
    fn init_script_delegates_from_lockdown_policy() {
        let policy = LockdownPolicy {
            block_new_windows: false,
            block_clipboard: true,
            block_context_menu: false,
            block_devtools: true,
            allow_reload: true,
            ..LockdownPolicy::default()
        };
        let js = build_init_script(&policy, false);

        assert!(js.contains("blockNewWindows: false,"));
        assert!(js.contains("blockClipboard: true,"));
        assert!(js.contains("blockContextMenu: false,"));
        assert!(js.contains("blockDevtools: true,"));
        assert!(js.contains("allowReload: true,"));
        assert!(js.contains("allowZoom: false,"));
        assert!(js.contains("allowFindInPage: false,"));
        assert!(js.contains("devMode: false,"));
    }

    #[test]
    fn init_script_hardens_ipc_a11y_and_clipboard_api() {
        let js = build_init_script(&LockdownPolicy::default(), false);
        assert!(js.contains("const postIpc = window.ipc && window.ipc.postMessage ? window.ipc.postMessage.bind(window.ipc) : null;"));
        assert!(js.contains("const safeStringify = JSON.stringify;"));
        assert!(js.contains("Object.defineProperty(window, '__examA11y'"));
        assert!(js.contains("value: Object.freeze("));
        assert!(js.contains("navigator.clipboard"));
        assert!(js.contains("readText:"));
        assert!(js.contains("writeText:"));
    }
}
