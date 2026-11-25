pub fn dashboard_html() -> &'static str {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>RLD Proctor Console</title>
<style>
  :root {
    --bg-rail: #F4F6F9;
    --bg-canvas: #FFFFFF;
    --bg-hover: #ECEFF4;
    --bg-row-flag: #FDF6F6;
    --border: rgba(25, 34, 48, 0.09);
    --border-strong: rgba(25, 34, 48, 0.16);
    --text-primary: #192230;
    --text-secondary: #4A5566;
    --text-muted: #7C889A;
    --accent: #235DBC;
    --ok: #1E7A56;
    --warn: #B26A14;
    --danger: #A62B36;
    --danger-hover: #8E222C;
    --font: "SF Pro", -apple-system, BlinkMacSystemFont, "Helvetica Neue", system-ui, sans-serif;
    --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace;
  }

  * { box-sizing: border-box; margin: 0; padding: 0; -webkit-font-smoothing: antialiased; }
  html, body {
    width: 100%;
    height: 100%;
    overflow: hidden;
    font-family: var(--font);
    background: var(--bg-canvas);
    color: var(--text-primary);
    font-size: 13px;
  }

  button:focus-visible, input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  /* Full-bleed macOS 3-pane console */
  .shell {
    display: flex;
    width: 100vw;
    height: 100vh;
  }

  /* Left Rail (248px) */
  .rail {
    width: 248px;
    flex-shrink: 0;
    background: var(--bg-rail);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    padding: 16px 12px;
    gap: 20px;
  }

  .rail-head {
    padding: 0 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .rail-title {
    font-size: 14px;
    font-weight: 600;
    line-height: 18px;
    color: var(--text-primary);
  }
  .rail-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .live-dot {
    width: 6px;
    height: 6px;
    border-radius: 999px;
    background: var(--ok);
    flex-shrink: 0;
  }

  .nav-group {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .section-label {
    padding: 0 8px 6px 8px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
  }
  .nav-item {
    appearance: none;
    width: 100%;
    height: 32px;
    padding: 0 8px;
    border-radius: 7px;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 13px;
    font-weight: 500;
    display: flex;
    align-items: center;
    justify-content: space-between;
    cursor: pointer;
    transition: background 100ms ease, color 100ms ease;
  }
  .nav-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
  .nav-item.active {
    background: var(--bg-canvas);
    border-color: var(--border);
    color: var(--text-primary);
  }
  .nav-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .nav-count {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .nav-count.alert { color: var(--danger); font-weight: 600; }

  .summary-list {
    padding: 0 8px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .summary-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 12px;
  }
  .summary-k { color: var(--text-secondary); }
  .summary-v {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text-primary);
    font-variant-numeric: tabular-nums;
  }
  .summary-v.alert { color: var(--danger); font-weight: 600; }
  .summary-key {
    margin-top: 4px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .summary-key-val {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-secondary);
    word-break: break-all;
  }

  .rail-footer {
    margin-top: auto;
    padding-top: 12px;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .rail-btn {
    appearance: none;
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border-radius: 7px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    color: var(--text-primary);
    font-family: var(--font);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    text-align: center;
  }
  .rail-btn:hover { background: var(--bg-hover); }
  .rail-btn.danger {
    background: #FDF6F6;
    color: var(--danger);
    border-color: rgba(166, 43, 54, 0.34);
    transition: background 100ms ease, border-color 100ms ease, color 100ms ease;
  }
  .rail-btn.danger:hover {
    background: var(--danger);
    border-color: var(--danger);
    color: #FFFFFF;
  }

  /* Center Workspace */
  .workspace {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-canvas);
  }

  .ws-bar {
    height: 48px;
    flex-shrink: 0;
    padding: 0 20px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .ws-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }
  .ws-controls {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .search {
    width: 200px;
    height: 28px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid var(--border-strong);
    background: var(--bg-rail);
    font-family: var(--font);
    font-size: 12px;
    color: var(--text-primary);
    outline: none;
  }
  .search:focus {
    background: var(--bg-canvas);
    border-color: var(--accent);
  }
  .density {
    position: relative;
    display: inline-flex;
    padding: 2px;
    background: var(--bg-rail);
    border: 1px solid var(--border-strong);
    border-radius: 7px;
  }
  .density-thumb {
    position: absolute;
    top: 2px;
    left: 0;
    height: 24px;
    border-radius: 5px;
    background: var(--bg-canvas);
    border: 1px solid var(--border);
    pointer-events: none;
    transition: transform 220ms cubic-bezier(0.22, 1, 0.36, 1), width 220ms cubic-bezier(0.22, 1, 0.36, 1);
    z-index: 0;
  }
  .density button {
    position: relative;
    z-index: 1;
    appearance: none;
    height: 24px;
    padding: 0 10px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    transition: color 140ms ease;
  }
  .density button.active {
    color: var(--text-primary);
  }

  .ws-scroll {
    flex: 1;
    overflow-y: auto;
  }

  /* Webcam Mosaic (fixed per-density width so opening Inspector never resizes tiles) */
  .cam-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, min(100%, var(--tile-w, 300px)));
    gap: 12px;
    padding: 16px 20px 24px 20px;
    justify-content: start;
  }
  .cam-tile {
    border: 1px solid var(--border-strong);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-canvas);
    cursor: pointer;
    transition: border-color 120ms ease;
  }
  .cam-tile:hover { border-color: rgba(25, 34, 48, 0.34); }
  .cam-tile.selected { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
  .cam-tile.flagged { border-color: var(--danger); box-shadow: 0 0 0 1px var(--danger); }

  .cam-frame {
    position: relative;
    width: 100%;
    aspect-ratio: 16 / 9;
    background: #141921;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #7C889A;
    font-size: 12px;
  }
  .cam-frame img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .cam-pip {
    position: absolute;
    right: 8px;
    bottom: 8px;
    width: 36%;
    aspect-ratio: 16 / 9;
    border-radius: 6px;
    overflow: hidden;
    background: #0E131B;
    border: 1.5px solid rgba(255, 255, 255, 0.28);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.48);
    pointer-events: none;
  }
  .cam-pip.lifted { bottom: 30px; }
  .cam-pip img { width: 100%; height: 100%; object-fit: cover; display: block; }
  .cam-flag-bar {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 4px 10px;
    background: rgba(166, 43, 54, 0.9);
    color: #FFFFFF;
    font-size: 11px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cam-foot {
    height: 36px;
    padding: 0 10px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    border-top: 1px solid var(--border);
  }
  .cam-id {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
  }
  .cam-name {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 0;
    max-width: 55%;
  }
  .cam-host {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .cam-tick {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    flex-shrink: 0;
  }

  /* Quiet macOS Roster Table (full-bleed, zero pill soup) */
  table { width: 100%; border-collapse: collapse; }
  th {
    position: sticky;
    top: 0;
    z-index: 2;
    text-align: left;
    height: 34px;
    padding: 0 16px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
    background: var(--bg-rail);
    border-bottom: 1px solid var(--border);
  }
  td {
    height: 44px;
    padding: 0 16px;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
    font-size: 13px;
  }
  tbody tr { cursor: pointer; }
  tbody tr:hover { background: var(--bg-rail); }
  tbody tr.selected { background: rgba(35, 93, 188, 0.06); }
  tbody tr.flagged { background: var(--bg-row-flag); }

  .dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 999px;
    margin-right: 7px;
    vertical-align: middle;
  }
  .dot-ok { background: var(--ok); }
  .dot-warn { background: var(--warn); }
  .dot-err { background: var(--danger); }
  .dot-muted { background: var(--text-muted); }

  .mono {
    font-family: var(--mono);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .muted { color: var(--text-muted); }
  .err-text { color: var(--danger); font-weight: 500; }

  .row-action {
    appearance: none;
    height: 26px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid rgba(166, 43, 54, 0.32);
    background: #FDF6F6;
    color: var(--danger);
    font-family: var(--font);
    font-size: 11.5px;
    font-weight: 500;
    cursor: pointer;
    transition: background 100ms ease, border-color 100ms ease, color 100ms ease;
  }
  .row-action:hover {
    background: var(--danger);
    border-color: var(--danger);
    color: #FFFFFF;
  }

  /* Right Inspector Pane (320px, opens on student click) */
  .inspector {
    width: 320px;
    flex-shrink: 0;
    background: var(--bg-rail);
    border-left: 1px solid var(--border);
    display: none;
    flex-direction: column;
  }
  .inspector.open { display: flex; }
  .insp-head {
    height: 48px;
    padding: 0 16px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .insp-name { font-size: 14px; font-weight: 600; }
  .insp-close {
    appearance: none;
    width: 26px;
    height: 26px;
    border-radius: 6px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .insp-close:hover { background: var(--bg-hover); color: var(--text-primary); }
  .insp-body {
    flex: 1;
    padding: 16px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .insp-cam {
    width: 100%;
    aspect-ratio: 16 / 9;
    border-radius: 8px;
    overflow: hidden;
    background: #141921;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #7C889A;
    font-size: 12px;
  }
  .insp-cam img { width: 100%; height: 100%; object-fit: cover; display: block; }
  .insp-screen-wrap {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .insp-screen-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 11px;
    font-weight: 500;
    letter-spacing: 0.03em;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .insp-screen-head .mono {
    text-transform: none;
    letter-spacing: 0;
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }
  .insp-screen {
    width: 100%;
    aspect-ratio: 16 / 9;
    border-radius: 8px;
    overflow: hidden;
    background: #141921;
    display: flex;
    align-items: center;
    justify-content: center;
    color: #7C889A;
    font-size: 12px;
  }
  .insp-screen img { width: 100%; height: 100%; object-fit: cover; display: block; }
  .insp-alert {
    padding: 10px 12px;
    border-radius: 8px;
    background: var(--bg-canvas);
    border: 1.5px solid var(--danger);
    font-size: 12px;
    line-height: 17px;
    color: var(--danger);
  }

  /* Live Room Settings View */
  .settings-wrap {
    max-width: 640px;
    padding: 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .settings-group {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
  }
  .setting-row {
    padding: 14px 0;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  .setting-title { font-size: 13px; font-weight: 500; color: var(--text-primary); }
  .setting-desc { font-size: 12px; color: var(--text-muted); margin-top: 2px; }
  .switch {
    appearance: none;
    position: relative;
    width: 34px;
    height: 20px;
    border-radius: 11px;
    border: none;
    background: #C7D0DE;
    cursor: pointer;
    transition: background 140ms ease;
    flex-shrink: 0;
  }
  .switch::after {
    content: "";
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 999px;
    background: #FFFFFF;
    transition: transform 140ms ease;
  }
  .switch[aria-checked="true"] { background: var(--accent); }
  .switch[aria-checked="true"]::after { transform: translateX(14px); }
  .insp-kv {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .insp-kv-row {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: 12px;
  }
  .insp-foot {
    padding: 12px 16px;
    border-top: 1px solid var(--border);
  }

  .empty {
    height: 100%;
    min-height: 360px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-secondary);
  }
  .empty-title { font-size: 14px; font-weight: 500; color: var(--text-primary); }
  .empty-sub { font-size: 12.5px; color: var(--text-muted); }

  /* Exam Settings View */
  .settings-wrap {
    max-width: 680px;
    padding: 20px 24px 40px 24px;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }
  .settings-section-title {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-muted);
    margin-bottom: 8px;
  }
  .settings-group {
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    background: var(--bg-canvas);
    overflow: hidden;
  }
  .setting-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .setting-row:last-child { border-bottom: none; }
  .setting-title { font-size: 13px; font-weight: 500; color: var(--text-primary); }
  .setting-desc { font-size: 11.5px; color: var(--text-muted); margin-top: 2px; line-height: 16px; }
  .switch {
    position: relative;
    width: 34px;
    height: 20px;
    flex-shrink: 0;
    border-radius: 999px;
    border: none;
    background: #C7D0DE;
    cursor: pointer;
    transition: background 150ms ease;
  }
  .switch::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 999px;
    background: #FFFFFF;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.16);
    transition: transform 150ms cubic-bezier(0.22, 1, 0.36, 1);
  }
  .switch[aria-checked="true"] { background: var(--accent); }
  .switch[aria-checked="true"]::after { transform: translateX(14px); }

  .list-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 9px 14px;
    border-bottom: 1px solid var(--border);
    font-size: 12.5px;
  }
  .list-item-main {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .list-item-url {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .remove-btn {
    appearance: none;
    height: 24px;
    padding: 0 8px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg-canvas);
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 11px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .remove-btn:hover { color: var(--danger); border-color: rgba(166, 43, 54, 0.35); background: var(--bg-row-flag); }
  .add-row {
    display: flex;
    gap: 8px;
    padding: 10px 14px;
    background: var(--bg-rail);
  }
  .add-input {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    border-radius: 6px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    font-family: var(--font);
    font-size: 12px;
    color: var(--text-primary);
    outline: none;
  }
  .add-input:focus { border-color: var(--accent); }
  .add-btn {
    appearance: none;
    height: 30px;
    padding: 0 12px;
    border-radius: 6px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: #FFFFFF;
    font-family: var(--font);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    flex-shrink: 0;
  }
  .add-btn:hover { opacity: 0.92; }
  .cred-current {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .cred-code {
    font-family: var(--mono);
    font-size: 12px;
    font-weight: 500;
    padding: 3px 8px;
    border-radius: 5px;
    background: var(--bg-rail);
    border: 1px solid var(--border-strong);
    color: var(--text-primary);
    letter-spacing: 0.02em;
    user-select: all;
  }
  .cred-copy {
    appearance: none;
    height: 24px;
    padding: 0 8px;
    border-radius: 5px;
    border: 1px solid var(--border);
    background: var(--bg-canvas);
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 11px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .cred-copy:hover {
    color: var(--text-primary);
    border-color: var(--accent);
  }
  .eye-btn {
    appearance: none;
    width: 32px;
    height: 30px;
    padding: 0;
    border-radius: 6px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    color: var(--text-secondary);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex-shrink: 0;
  }
  .eye-btn:hover {
    color: var(--text-primary);
    border-color: rgba(25, 34, 48, 0.3);
  }
  .eye-btn[aria-pressed="true"] {
    color: var(--accent);
    border-color: var(--accent);
    background: rgba(35, 93, 188, 0.06);
  }
  .preset-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 14px 10px 14px;
    background: var(--bg-rail);
    border-top: 1px solid var(--border);
  }
  .preset-btn {
    appearance: none;
    height: 24px;
    padding: 0 9px;
    border-radius: 5px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 11px;
    cursor: pointer;
  }
  .preset-btn:hover { color: var(--text-primary); border-color: var(--accent); }

  /* Confirmation Modal */
  .modal-backdrop {
    display: none;
    position: fixed;
    inset: 0;
    background: rgba(25, 34, 48, 0.24);
    backdrop-filter: blur(4px);
    align-items: center;
    justify-content: center;
    z-index: 50;
  }
  .modal-backdrop.open { display: flex; }
  .modal-card {
    width: 384px;
    background: var(--bg-canvas);
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    box-shadow: 0 18px 40px rgba(25, 34, 48, 0.16);
    overflow: hidden;
  }
  .modal-body { padding: 20px 24px 16px 24px; }
  .modal-body h2 { font-size: 17px; font-weight: 600; margin-bottom: 8px; }
  .modal-body p { font-size: 13px; line-height: 19px; color: var(--text-secondary); }
  .modal-foot {
    padding: 12px 24px;
    border-top: 1px solid var(--border);
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn-plain {
    appearance: none;
    height: 30px;
    padding: 0 12px;
    border-radius: 7px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    color: var(--text-primary);
    font-family: var(--font);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }
  .btn-danger-solid {
    appearance: none;
    height: 30px;
    padding: 0 12px;
    border-radius: 7px;
    border: 1px solid var(--danger);
    background: var(--danger);
    color: #FFFFFF;
    font-family: var(--font);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
  }
  .btn-danger-solid:hover { background: var(--danger-hover); }

  /* Teacher Sign-In / Create Account View (Quiet Native 396px card) */
  .auth-view {
    display: none;
    position: fixed;
    inset: 0;
    background: #F6F7F9;
    align-items: center;
    justify-content: center;
    padding: 24px;
    z-index: 40;
  }
  .auth-view.open { display: flex; }
  .auth-card {
    width: 100%;
    max-width: 396px;
    background: #FFFFFF;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    box-shadow: 0 10px 30px rgba(25, 34, 48, 0.06);
    padding: 28px 28px 24px 28px;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .auth-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }
  .auth-brand {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }
  .auth-badge {
    font-size: 11px;
    font-weight: 500;
    color: var(--text-secondary);
    background: var(--bg-rail);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 8px;
  }
  .auth-seg {
    display: grid;
    grid-template-columns: 1fr 1fr;
    padding: 2px;
    background: var(--bg-rail);
    border: 1px solid var(--border-strong);
    border-radius: 8px;
  }
  .auth-seg button {
    appearance: none;
    height: 28px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease;
  }
  .auth-seg button.active {
    background: var(--bg-canvas);
    border-color: var(--border);
    color: var(--text-primary);
  }
  .auth-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .auth-field {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .auth-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
  }
  .auth-input-row {
    display: flex;
    gap: 6px;
  }
  .auth-input {
    width: 100%;
    height: 34px;
    padding: 0 10px;
    border-radius: 7px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    color: var(--text-primary);
    font-family: var(--font);
    font-size: 13px;
    outline: none;
  }
  .auth-input:focus { border-color: var(--accent); }
  .auth-error {
    display: none;
    font-size: 12px;
    line-height: 17px;
    color: var(--danger);
    background: var(--bg-row-flag);
    border: 1px solid rgba(166, 43, 54, 0.25);
    border-radius: 7px;
    padding: 8px 10px;
  }
  .auth-error.show { display: block; }
  .auth-submit {
    appearance: none;
    width: 100%;
    height: 36px;
    margin-top: 2px;
    border-radius: 8px;
    border: 1px solid var(--accent);
    background: var(--accent);
    color: #FFFFFF;
    font-family: var(--font);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
  }
  .auth-submit:hover { opacity: 0.93; }
  .auth-submit:disabled { opacity: 0.6; cursor: default; }

  /* Teacher account summary in rail footer */
  .rail-account {
    padding: 4px 4px 8px 4px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 4px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .rail-account-meta {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .rail-account-name {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rail-account-email {
    font-size: 11px;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .rail-signout {
    appearance: none;
    height: 24px;
    padding: 0 8px;
    border-radius: 5px;
    border: 1px solid var(--border-strong);
    background: var(--bg-canvas);
    color: var(--text-secondary);
    font-family: var(--font);
    font-size: 11px;
    font-weight: 500;
    cursor: pointer;
    flex-shrink: 0;
  }
  .rail-signout:hover { color: var(--text-primary); background: var(--bg-hover); }

  /* Exam Rooms View */
  .rooms-bar {
    padding: 12px 20px;
    background: var(--bg-rail);
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .rooms-err {
    font-size: 12px;
    color: var(--danger);
    padding: 8px 20px 0 20px;
  }
  .room-actions {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    justify-content: flex-end;
  }
  .row-action.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: #FFFFFF;
  }
  .row-action.primary:hover { opacity: 0.92; color: #FFFFFF; }
  .row-action.danger:hover {
    color: var(--danger);
    border-color: rgba(166, 43, 54, 0.35);
    background: var(--bg-row-flag);
  }
</style>
</head>
<body>
  <div class="auth-view" id="auth-view" aria-label="Teacher sign in">
    <div class="auth-card">
      <div class="auth-head">
        <div class="auth-brand">RLD Proctor</div>
        <span class="auth-badge">Teacher Console</span>
      </div>
      <div class="auth-seg" role="tablist" aria-label="Authentication mode">
        <button type="button" id="auth-tab-login" class="active" role="tab" aria-selected="true">Sign in</button>
        <button type="button" id="auth-tab-register" role="tab" aria-selected="false">Create account</button>
      </div>
      <form class="auth-form" id="auth-form" novalidate>
        <div class="auth-field" id="auth-name-field" style="display:none;">
          <label class="auth-label" for="auth-name">Full name</label>
          <input type="text" class="auth-input" id="auth-name" placeholder="Ms. Lovelace" autocomplete="name" />
        </div>
        <div class="auth-field">
          <label class="auth-label" for="auth-email">School email</label>
          <input type="email" class="auth-input" id="auth-email" placeholder="teacher@school.edu" autocomplete="email" required />
        </div>
        <div class="auth-field">
          <label class="auth-label" for="auth-password">Password</label>
          <div class="auth-input-row">
            <input type="password" class="auth-input" id="auth-password" placeholder="At least 6 characters" autocomplete="current-password" required />
            <button type="button" class="eye-btn" id="auth-eye-btn" style="height:34px;width:34px;" aria-label="Show password" aria-pressed="false" title="Show"></button>
          </div>
        </div>
        <div class="auth-error" id="auth-error" role="alert"></div>
        <button type="submit" class="auth-submit" id="auth-submit-btn">Sign in</button>
      </form>
    </div>
  </div>

  <div class="shell" id="app-shell" style="display:none;">
    <aside class="rail">
      <div class="rail-head">
        <div class="rail-title">RLD Proctor</div>
        <div class="rail-meta">
          <span class="live-dot"></span>
          <span id="room-label">AP-CS-P2</span>
        </div>
      </div>

      <nav class="nav-group" aria-label="Views">
        <div class="section-label">Monitor</div>
        <button type="button" class="nav-item active" data-view="cameras">
          <span class="nav-left">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="14" height="14" rx="2"/><path d="m17 10 4-2.5v9L17 14"/></svg>
            <span>Cameras</span>
          </span>
          <span class="nav-count" id="cnt-cams">0</span>
        </button>
        <button type="button" class="nav-item" data-view="roster">
          <span class="nav-left">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M4 7h16M4 12h16M4 17h10"/></svg>
            <span>Roster</span>
          </span>
          <span class="nav-count" id="cnt-all">0</span>
        </button>
        <button type="button" class="nav-item" data-view="attention">
          <span class="nav-left">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4 3 19h18L12 4Zm0 6v4m0 3h.01"/></svg>
            <span>Needs attention</span>
          </span>
          <span class="nav-count" id="cnt-flag">0</span>
        </button>
        <button type="button" class="nav-item" data-view="rooms">
          <span class="nav-left">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M14 10h3M14 14h3"/></svg>
            <span>Rooms</span>
          </span>
          <span class="nav-count" id="cnt-rooms">0</span>
        </button>
        <button type="button" class="nav-item" data-view="settings">
          <span class="nav-left">
            <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9c.26.6.85 1 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z"/></svg>
            <span>Exam settings</span>
          </span>
        </button>
      </nav>

      <div class="summary-list">
        <div class="section-label" style="padding:0 0 2px 0;">Session</div>
        <div class="summary-row"><span class="summary-k">In exam</span><span class="summary-v" id="sum-active">0</span></div>
        <div class="summary-row"><span class="summary-k">Flagged</span><span class="summary-v" id="sum-flagged">0</span></div>
        <div class="summary-row"><span class="summary-k">Total connected</span><span class="summary-v" id="sum-total">0</span></div>
        <div class="summary-key">
          <span class="summary-k">Expected config key</span>
          <span class="summary-key-val" id="sum-key">Unpinned</span>
        </div>
      </div>

      <div class="rail-footer">
        <div class="rail-account" id="rail-account">
          <div class="rail-account-meta">
            <div class="rail-account-name" id="teacher-name">Local Proctor</div>
            <div class="rail-account-email" id="teacher-email">Token session</div>
          </div>
          <button type="button" class="rail-signout" id="sign-out-btn">Sign out</button>
        </div>
        <button type="button" class="rail-btn" id="copy-url-btn">Copy dashboard URL</button>
        <button type="button" class="rail-btn danger" id="unlock-all-btn">End exam for all</button>
      </div>
    </aside>

    <main class="workspace">
      <header class="ws-bar">
        <div class="ws-title" id="ws-title">Cameras</div>
        <div class="ws-controls">
          <div class="density" id="density-ctrl" role="group" aria-label="Grid size">
            <div class="density-thumb" id="density-thumb" aria-hidden="true"></div>
            <button type="button" data-min="220">Compact</button>
            <button type="button" data-min="300" class="active">Standard</button>
            <button type="button" data-min="400">Large</button>
          </div>
          <input type="text" class="search" id="search-input" placeholder="Filter student" aria-label="Filter student" />
        </div>
      </header>
      <div class="ws-scroll" id="content-wrap">
        <div class="empty">
          <div class="empty-title">Waiting for students</div>
          <div class="empty-sub">Connected exam sessions appear here automatically</div>
        </div>
      </div>
    </main>

    <aside class="inspector" id="inspector" aria-label="Student inspector">
      <div class="insp-head">
        <div class="insp-name" id="insp-name">Student</div>
        <button type="button" class="insp-close" id="insp-close" aria-label="Close inspector">
          <svg viewBox="0 0 20 20" width="14" height="14"><path d="M5 5l10 10M15 5L5 15" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>
        </button>
      </div>
      <div class="insp-body" id="insp-body"></div>
      <div class="insp-foot">
        <button type="button" class="rail-btn danger" id="insp-unlock-btn" title="Remotely exit lockdown for this student">End student session</button>
      </div>
    </aside>
  </div>

  <div class="modal-backdrop" id="end-all-modal" role="dialog" aria-modal="true">
    <div class="modal-card">
      <div class="modal-body">
        <h2 id="end-all-title">End exam for all students?</h2>
        <p>Every connected MacBook in this room will exit lockdown on its next heartbeat. Make sure students have submitted their work first.</p>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn-plain" id="cancel-end-all">Cancel</button>
        <button type="button" class="btn-danger-solid" id="confirm-end-all">End exam for all</button>
      </div>
    </div>
  </div>

<script>
  let currentView = 'cameras';
  let selectedId = null;
  let tileWidth = 300;
  let latestState = null;
  let latestCredentials = { entrance_code: null, quit_password: null };
  let latestRooms = { active_room: '', rooms: [] };
  let lastCamGridSig = '';
  let lastInspSig = '';
  let lastSettingsSig = '';
  let lastRoomsSig = '';
  let roomsDraftName = '';
  let roomsError = '';
  let isAuthenticated = false;
  let authMode = 'login';
  let authPasswordVisible = false;
  let pollTimer = null;
  let mediaWs = null;
  let wsReconnectTimer = null;
  const mediaUrls = new Map();
  const wsDecoder = new TextDecoder();
  const credEyeVisible = { entrance: false, quit: false };
  const settingsDrafts = { entrance: '', quit: '', allowed: '' };

  const eyeSvg = (visible) => visible
    ? '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.94 10.94 0 0 1 12 20c-7 0-11-8-11-8a21.77 21.77 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a21.8 21.8 0 0 1-2.16 3.19"/><path d="M14.12 14.12a3 3 0 1 1-4.24-4.24"/><path d="M1 1l22 22"/></svg>'
    : '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>';

  const esc = (s) => String(s ?? '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;').replace(/'/g,'&#39;');
  const isFlagged = (s) => !s.config_key_verified || (s.violations && s.violations.length > 0) || s.status === 'security_paused';

  const sendFocus = (studentId) => {
    if (mediaWs && mediaWs.readyState === WebSocket.OPEN) {
      try {
        mediaWs.send(JSON.stringify({ type: 'focus', student_id: studentId || null }));
      } catch (_) {}
    }
  };

  const setSelectedId = (id) => {
    const next = id || null;
    if (selectedId !== next) {
      selectedId = next;
      sendFocus(selectedId);
    }
  };

  const disconnectMediaWs = () => {
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    if (mediaWs) {
      const ws = mediaWs;
      mediaWs = null;
      ws.onclose = null;
      ws.onerror = null;
      ws.onmessage = null;
      try { ws.close(); } catch (_) {}
    }
    for (const url of mediaUrls.values()) {
      try { URL.revokeObjectURL(url); } catch (_) {}
    }
    mediaUrls.clear();
  };

  const connectMediaWs = () => {
    if (!isAuthenticated) return;
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    if (mediaWs && (mediaWs.readyState === WebSocket.OPEN || mediaWs.readyState === WebSocket.CONNECTING)) {
      return;
    }
    const proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
    const ws = new WebSocket(proto + location.host + '/ws/proctor');
    ws.binaryType = 'arraybuffer';
    mediaWs = ws;

    ws.onopen = () => {
      sendFocus(selectedId);
    };

    ws.onmessage = (ev) => {
      if (!(ev.data instanceof ArrayBuffer)) return;
      const bytes = new Uint8Array(ev.data);
      if (bytes.length < 3) return;
      const kind = bytes[0];
      const idLen = bytes[1];
      if (bytes.length <= 2 + idLen) return;
      const studentId = wsDecoder.decode(bytes.subarray(2, 2 + idLen));
      const jpeg = bytes.subarray(2 + idLen);
      const blob = new Blob([jpeg], { type: 'image/jpeg' });
      const url = URL.createObjectURL(blob);
      const key = `${kind}:${studentId}`;
      const prevUrl = mediaUrls.get(key);
      mediaUrls.set(key, url);
      const attr = kind === 0 ? 'data-cam-img' : 'data-screen-img';
      document.querySelectorAll(`img[${attr}="${CSS.escape(studentId)}"]`).forEach((img) => {
        img.src = url;
      });
      if (prevUrl) {
        try { URL.revokeObjectURL(prevUrl); } catch (_) {}
      }
    };

    ws.onclose = () => {
      if (mediaWs === ws) mediaWs = null;
      if (isAuthenticated && !wsReconnectTimer) {
        wsReconnectTimer = setTimeout(() => {
          wsReconnectTimer = null;
          connectMediaWs();
        }, 1500);
      }
    };

    ws.onerror = () => {
      try { ws.close(); } catch (_) {}
    };
  };

  const authView = document.getElementById('auth-view');
  const appShell = document.getElementById('app-shell');
  const authTabLogin = document.getElementById('auth-tab-login');
  const authTabRegister = document.getElementById('auth-tab-register');
  const authNameField = document.getElementById('auth-name-field');
  const authNameInput = document.getElementById('auth-name');
  const authEmailInput = document.getElementById('auth-email');
  const authPasswordInput = document.getElementById('auth-password');
  const authEyeBtn = document.getElementById('auth-eye-btn');
  const authErrorEl = document.getElementById('auth-error');
  const authSubmitBtn = document.getElementById('auth-submit-btn');

  const setAuthError = (msg) => {
    if (msg) {
      authErrorEl.textContent = msg;
      authErrorEl.classList.add('show');
    } else {
      authErrorEl.textContent = '';
      authErrorEl.classList.remove('show');
    }
  };

  const setAuthMode = (mode) => {
    authMode = mode === 'register' ? 'register' : 'login';
    const isReg = authMode === 'register';
    authTabLogin.classList.toggle('active', !isReg);
    authTabLogin.setAttribute('aria-selected', isReg ? 'false' : 'true');
    authTabRegister.classList.toggle('active', isReg);
    authTabRegister.setAttribute('aria-selected', isReg ? 'true' : 'false');
    authNameField.style.display = isReg ? 'flex' : 'none';
    authPasswordInput.setAttribute('autocomplete', isReg ? 'new-password' : 'current-password');
    authSubmitBtn.textContent = isReg ? 'Create account' : 'Sign in';
    setAuthError('');
  };

  authEyeBtn.innerHTML = eyeSvg(false);
  authEyeBtn.addEventListener('click', () => {
    authPasswordVisible = !authPasswordVisible;
    authPasswordInput.type = authPasswordVisible ? 'text' : 'password';
    authEyeBtn.setAttribute('aria-pressed', authPasswordVisible ? 'true' : 'false');
    authEyeBtn.setAttribute('aria-label', authPasswordVisible ? 'Hide password' : 'Show password');
    authEyeBtn.title = authPasswordVisible ? 'Hide' : 'Show';
    authEyeBtn.innerHTML = eyeSvg(authPasswordVisible);
  });

  authTabLogin.addEventListener('click', () => setAuthMode('login'));
  authTabRegister.addEventListener('click', () => setAuthMode('register'));

  const showAuthScreen = (hasAccounts) => {
    isAuthenticated = false;
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
    disconnectMediaWs();
    setSelectedId(null);
    appShell.style.display = 'none';
    authView.classList.add('open');
    setAuthMode(hasAccounts ? 'login' : 'register');
  };

  const enterConsole = (me) => {
    isAuthenticated = true;
    authView.classList.remove('open');
    appShell.style.display = 'flex';
    const teacherNameEl = document.getElementById('teacher-name');
    const teacherEmailEl = document.getElementById('teacher-email');
    if (me && me.teacher) {
      teacherNameEl.textContent = me.teacher.name || 'Teacher';
      teacherEmailEl.textContent = me.teacher.email || '';
    } else {
      teacherNameEl.textContent = 'Local Proctor';
      teacherEmailEl.textContent = 'Token session';
    }
    requestAnimationFrame(() => syncDensityThumb(false));
    connectMediaWs();
    refresh(true);
    if (!pollTimer) {
      pollTimer = setInterval(refresh, 1200);
    }
  };

  document.getElementById('auth-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    setAuthError('');
    const isReg = authMode === 'register';
    const payload = isReg
      ? {
          name: authNameInput.value.trim(),
          email: authEmailInput.value.trim(),
          password: authPasswordInput.value,
        }
      : {
          email: authEmailInput.value.trim(),
          password: authPasswordInput.value,
        };
    authSubmitBtn.disabled = true;
    try {
      const res = await fetch(isReg ? '/api/auth/register' : '/api/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) {
        setAuthError(data.error || 'Authentication failed');
        return;
      }
      authPasswordInput.value = '';
      enterConsole({ authenticated: true, teacher: data.teacher || null });
    } catch (_) {
      setAuthError('Unable to reach proctor server');
    } finally {
      authSubmitBtn.disabled = false;
    }
  });

  document.getElementById('sign-out-btn').addEventListener('click', async () => {
    try {
      await fetch('/api/auth/logout', { method: 'POST' });
    } catch (_) {}
    latestState = null;
    latestRooms = { active_room: '', rooms: [] };
    lastCamGridSig = '';
    lastInspSig = '';
    lastSettingsSig = '';
    lastRoomsSig = '';
    showAuthScreen(true);
  });

  const statusMeta = (status) => {
    switch (status) {
      case 'in_exam': return ['dot-ok', 'In exam'];
      case 'waiting_for_code': return ['dot-muted', 'Waiting for code'];
      case 'preflight_blocked': return ['dot-warn', 'Preflight check'];
      case 'security_paused': return ['dot-err', 'Paused'];
      case 'exited': return ['dot-muted', 'Exited'];
      default: return ['dot-muted', status];
    }
  };

  const unlockOne = async (studentId) => {
    await fetch('/api/unlock', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ student_id: studentId }),
    });
    await refresh();
  };

  const densityThumb = document.getElementById('density-thumb');
  const syncDensityThumb = (animate = true) => {
    if (!densityThumb) return;
    const active = document.querySelector('#density-ctrl button.active');
    if (!active) return;
    if (!animate) {
      const prev = densityThumb.style.transition;
      densityThumb.style.transition = 'none';
      densityThumb.style.width = active.offsetWidth + 'px';
      densityThumb.style.transform = `translateX(${active.offsetLeft}px)`;
      void densityThumb.offsetWidth;
      densityThumb.style.transition = prev;
    } else {
      densityThumb.style.width = active.offsetWidth + 'px';
      densityThumb.style.transform = `translateX(${active.offsetLeft}px)`;
    }
  };
  requestAnimationFrame(() => syncDensityThumb(false));

  const savePolicy = async (patch) => {
    const base = (latestState && latestState.policy) || {
      allowed_domains: [],
      blocked_domains: [],
      pinned_tabs: [],
      require_webcam: true,
      allow_read_aloud: false,
      allow_find_in_page: false,
      allow_back_forward: false,
      allow_reload: true,
      allow_zoom: true,
      block_downloads: true,
      block_multiple_displays: true,
      block_screen_capture_apps: true,
    };
    const next = Object.assign({}, base, patch);
    await fetch('/api/policy', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(next),
    });
    await refresh(true);
  };

  const saveCredential = async (kind, value) => {
    const res = await fetch('/api/credentials', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ kind, value }),
    });
    if (res.ok) {
      if (kind === 'entrance_code') settingsDrafts.entrance = '';
      if (kind === 'quit_password') settingsDrafts.quit = '';
    }
    lastSettingsSig = '';
    await refresh(true);
  };

  const switchView = (view) => {
    currentView = view;
    document.querySelectorAll('.nav-item').forEach((b) => b.classList.toggle('active', b.dataset.view === view));
    const titles = { cameras: 'Cameras', roster: 'Roster', attention: 'Needs attention', rooms: 'Exam rooms', settings: 'Exam settings' };
    document.getElementById('ws-title').textContent = titles[currentView] || 'Monitor';
    document.getElementById('density-ctrl').style.display = currentView === 'cameras' ? 'inline-flex' : 'none';
    document.getElementById('search-input').style.display = (currentView === 'settings' || currentView === 'rooms') ? 'none' : 'block';
    if (currentView === 'cameras') requestAnimationFrame(() => syncDensityThumb(false));
    render();
  };

  document.querySelectorAll('.nav-item').forEach((btn) => {
    btn.addEventListener('click', () => switchView(btn.dataset.view));
  });

  document.querySelectorAll('#density-ctrl button').forEach((btn) => {
    btn.addEventListener('click', () => {
      tileWidth = Number(btn.dataset.min) || 300;
      document.querySelectorAll('#density-ctrl button').forEach((b) => b.classList.toggle('active', b === btn));
      syncDensityThumb(true);
      render();
    });
  });

  document.getElementById('search-input').addEventListener('input', () => render());

  document.getElementById('copy-url-btn').addEventListener('click', async () => {
    const btn = document.getElementById('copy-url-btn');
    try {
      await navigator.clipboard.writeText(window.location.href);
      btn.textContent = 'Copied URL';
      setTimeout(() => (btn.textContent = 'Copy dashboard URL'), 1500);
    } catch (_) {}
  });

  const endModal = document.getElementById('end-all-modal');
  document.getElementById('unlock-all-btn').addEventListener('click', () => {
    const n = latestState ? latestState.total_students : 0;
    document.getElementById('end-all-title').textContent = `End exam for all ${n} student(s)?`;
    endModal.classList.add('open');
  });
  document.getElementById('cancel-end-all').addEventListener('click', () => endModal.classList.remove('open'));
  document.getElementById('confirm-end-all').addEventListener('click', async () => {
    endModal.classList.remove('open');
    await fetch('/api/unlock-all', { method: 'POST' });
    await refresh();
  });

  document.getElementById('insp-close').addEventListener('click', () => {
    setSelectedId(null);
    document.getElementById('inspector').classList.remove('open');
    render();
  });
  document.getElementById('insp-unlock-btn').addEventListener('click', () => {
    if (selectedId) unlockOne(selectedId);
  });

  const renderInspector = () => {
    const panel = document.getElementById('inspector');
    if (!selectedId || !latestState) {
      panel.classList.remove('open');
      lastInspSig = '';
      return;
    }
    const s = (latestState.students || []).find((x) => x.student_id === selectedId);
    if (!s) {
      setSelectedId(null);
      panel.classList.remove('open');
      lastInspSig = '';
      return;
    }
    panel.classList.add('open');
    document.getElementById('insp-name').textContent = s.student_id;
    const inspSig = JSON.stringify([s.student_id, s.status, s.config_key_verified, s.violations, s.has_webcam, s.has_screen]);
    if (inspSig === lastInspSig) {
      const hostPill = document.getElementById('insp-host-pill');
      if (hostPill) hostPill.textContent = s.active_host || '—';
      const activeHost = document.getElementById('insp-active-host');
      if (activeHost) activeHost.textContent = s.active_host || '—';
      const seenEl = document.getElementById('insp-last-seen');
      if (seenEl) seenEl.textContent = `${s.last_seen_secs_ago}s ago`;
      return;
    }
    lastInspSig = inspSig;

    const [dotCls, label] = statusMeta(s.status);
    const camSrc = mediaUrls.get(`0:${s.student_id}`) || '';
    const screenSrc = mediaUrls.get(`1:${s.student_id}`) || '';
    const camHtml = s.status === 'exited'
      ? `<span data-cam-placeholder="${esc(s.student_id)}">Session ended — camera closed</span>`
      : (s.has_webcam
        ? `<img data-cam-img="${esc(s.student_id)}"${camSrc ? ` src="${camSrc}"` : ''} alt="${esc(s.student_id)}" />`
        : `<span data-cam-placeholder="${esc(s.student_id)}">Camera off</span>`);
    const screenHtml = s.status === 'exited'
      ? '<span>Session ended</span>'
      : (s.has_screen
        ? `<img data-screen-img="${esc(s.student_id)}"${screenSrc ? ` src="${screenSrc}"` : ''} alt="Student live screen" />`
        : '<span>Waiting for screen…</span>');
    const alertHtml = (s.violations && s.violations.length > 0)
      ? `<div class="insp-alert">${s.violations.map(esc).join('<br/>')}</div>`
      : (!s.config_key_verified ? '<div class="insp-alert">Config key does not match room policy</div>' : '');

    document.getElementById('insp-body').innerHTML = `
      <div class="insp-cam" data-cam-box="${esc(s.student_id)}">${camHtml}</div>
      <div class="insp-screen-wrap">
        <div class="insp-screen-head">
          <span>Live screen</span>
          <span class="mono" id="insp-host-pill">${esc(s.active_host || '—')}</span>
        </div>
        <div class="insp-screen">${screenHtml}</div>
      </div>
      ${alertHtml}
      <div class="insp-kv">
        <div class="insp-kv-row"><span class="muted">Status</span><span><span class="dot ${dotCls}"></span>${esc(label)}</span></div>
        <div class="insp-kv-row"><span class="muted">Config key</span><span class="mono ${s.config_key_verified ? '' : 'err-text'}">${s.config_key_verified ? 'Verified' : 'Mismatch'}</span></div>
        <div class="insp-kv-row"><span class="muted">Active host</span><span class="mono" id="insp-active-host">${esc(s.active_host || '—')}</span></div>
        <div class="insp-kv-row"><span class="muted">Last seen</span><span class="mono" id="insp-last-seen">${s.last_seen_secs_ago}s ago</span></div>
      </div>
    `;
  };

  const render = () => {
    if (!latestState) return;
    const wrap = document.getElementById('content-wrap');

    if (currentView === 'rooms') {
      lastCamGridSig = '';
      lastSettingsSig = '';
      const rooms = Array.isArray(latestRooms.rooms) ? latestRooms.rooms : [];
      const activeRoom = latestRooms.active_room || (latestState && latestState.room_code) || '';
      const roomsSig = JSON.stringify([activeRoom, rooms, roomsError]);
      if (roomsSig === lastRoomsSig) {
        renderInspector();
        return;
      }

      const prevRoomInput = document.getElementById('new-room-name');
      if (prevRoomInput) roomsDraftName = prevRoomInput.value;
      const activeEl = document.activeElement;
      const hadInputFocus = activeEl && activeEl.id === 'new-room-name';
      const prevSelStart = hadInputFocus && typeof activeEl.selectionStart === 'number' ? activeEl.selectionStart : null;
      const prevSelEnd = hadInputFocus && typeof activeEl.selectionEnd === 'number' ? activeEl.selectionEnd : null;

      lastRoomsSig = roomsSig;
      const rowsHtml = rooms.length
        ? `<table>
            <thead><tr>
              <th>Room</th><th>Code</th><th style="text-align:right">Actions</th>
            </tr></thead>
            <tbody>${rooms.map((r) => {
              const isAct = r.code === activeRoom;
              return `<tr class="${isAct ? 'selected' : ''}">
                <td><strong>${esc(r.name)}</strong>${isAct ? ' <span class="dot dot-ok" style="margin-left:8px;margin-right:4px"></span><span class="muted" style="font-size:11.5px">Active</span>' : ''}</td>
                <td class="mono">${esc(r.code)}</td>
                <td style="text-align:right">
                  <div class="room-actions">
                    <button type="button" class="row-action" data-copy-rld="${esc(r.code)}">Copy rld:// link</button>
                    <button type="button" class="row-action primary" data-open-room="${esc(r.code)}">Open</button>
                    <button type="button" class="row-action danger" data-del-room="${esc(r.code)}">Delete</button>
                  </div>
                </td>
              </tr>`;
            }).join('')}</tbody>
          </table>`
        : `<div class="empty">
            <div class="empty-title">No exam rooms yet</div>
            <div class="empty-sub">Create a room above to generate a room code for students</div>
          </div>`;

      wrap.innerHTML = `
        <div class="rooms-bar">
          <input type="text" class="add-input" id="new-room-name" placeholder="Room name (e.g. AP CS Period 2)" />
          <button type="button" class="add-btn" id="create-room-btn">New room</button>
        </div>
        ${roomsError ? `<div class="rooms-err">${esc(roomsError)}</div>` : ''}
        ${rowsHtml}
      `;

      const nameInput = document.getElementById('new-room-name');
      nameInput.value = roomsDraftName;
      nameInput.addEventListener('input', () => { roomsDraftName = nameInput.value; });

      const createRoom = async () => {
        const name = nameInput.value.trim();
        if (!name) {
          roomsError = 'Enter a room name';
          lastRoomsSig = '';
          render();
          return;
        }
        roomsError = '';
        const res = await fetch('/api/rooms', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ name }),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
          roomsError = data.error || 'Failed to create room';
          lastRoomsSig = '';
          render();
          return;
        }
        roomsDraftName = '';
        lastRoomsSig = '';
        await refresh(true);
      };

      document.getElementById('create-room-btn').addEventListener('click', createRoom);
      nameInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') createRoom(); });

      wrap.querySelectorAll('[data-copy-rld]').forEach((btn) => {
        btn.addEventListener('click', async () => {
          const code = btn.dataset.copyRld;
          const link = `rld://join?proctor=${encodeURIComponent(location.origin)}&room=${encodeURIComponent(code)}`;
          try {
            await navigator.clipboard.writeText(link);
            btn.textContent = 'Copied link';
            setTimeout(() => { btn.textContent = 'Copy rld:// link'; }, 1400);
          } catch (_) {}
        });
      });

      wrap.querySelectorAll('[data-open-room]').forEach((btn) => {
        btn.addEventListener('click', async () => {
          const code = btn.dataset.openRoom;
          await fetch('/api/rooms/select', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ code }),
          });
          lastRoomsSig = '';
          await refresh(true);
          switchView('cameras');
        });
      });

      wrap.querySelectorAll('[data-del-room]').forEach((btn) => {
        btn.addEventListener('click', async () => {
          const code = btn.dataset.delRoom;
          await fetch(`/api/rooms/${encodeURIComponent(code)}`, { method: 'DELETE' });
          lastRoomsSig = '';
          await refresh(true);
        });
      });

      if (hadInputFocus) {
        nameInput.focus();
        if (prevSelStart !== null && prevSelEnd !== null) {
          try { nameInput.setSelectionRange(prevSelStart, prevSelEnd); } catch (_) {}
        }
      }

      renderInspector();
      return;
    }

    lastRoomsSig = '';
    if (currentView === 'settings') {
      lastCamGridSig = '';
      const p = latestState.policy || {};
      const settingsSig = JSON.stringify([p, latestCredentials]);
      if (settingsSig === lastSettingsSig) {
        renderInspector();
        return;
      }

      const prevEnt = document.getElementById('entrance-code-input');
      const prevQuit = document.getElementById('quit-pwd-input');
      const prevAllowed = document.getElementById('add-allowed-input');
      if (prevEnt) settingsDrafts.entrance = prevEnt.value;
      if (prevQuit) settingsDrafts.quit = prevQuit.value;
      if (prevAllowed) settingsDrafts.allowed = prevAllowed.value;
      const activeEl = document.activeElement;
      const prevFocusId = activeEl && activeEl.closest && activeEl.closest('#content-wrap') ? activeEl.id : null;
      const prevSelStart = activeEl && typeof activeEl.selectionStart === 'number' ? activeEl.selectionStart : null;
      const prevSelEnd = activeEl && typeof activeEl.selectionEnd === 'number' ? activeEl.selectionEnd : null;

      lastSettingsSig = settingsSig;
      const allowed = Array.isArray(p.allowed_domains) ? p.allowed_domains : [];

      const row = (key, title, desc, checked) => `<div class="setting-row">
        <div><div class="setting-title">${title}</div><div class="setting-desc">${desc}</div></div>
        <button type="button" class="switch" role="switch" aria-checked="${checked ? 'true' : 'false'}" data-policy-key="${key}" aria-label="${title}"></button>
      </div>`;

      const allowedRows = allowed.length
        ? allowed.map((d, i) => `<div class="list-item"><span class="mono">${esc(d)}</span><button type="button" class="remove-btn" data-rm-allowed="${i}">Remove</button></div>`).join('')
        : '<div class="list-item"><span class="muted">Only the exam start domain is allowed</span></div>';

      const hasEntrance = Boolean(p.entrance_code_hash);
      const hasQuit = Boolean(p.quit_password_hash);
      const curEntrance = latestCredentials && latestCredentials.entrance_code;
      const curQuit = latestCredentials && latestCredentials.quit_password;

      const renderCredBadge = (hasHash, plain, kind) => {
        if (!hasHash) return '<span class="mono muted">Off</span>';
        if (plain) {
          return `<span class="cred-current">
            <code class="cred-code">${esc(plain)}</code>
            <button type="button" class="cred-copy" data-copy-cred="${kind}" title="Copy to clipboard">Copy</button>
          </span>`;
        }
        return '<span class="mono">Set</span>';
      };

      wrap.innerHTML = `<div class="settings-wrap">
        <div>
          <div style="font-size:15px;font-weight:600;margin-bottom:4px;">Live classroom rules</div>
          <div style="font-size:12.5px;color:var(--text-muted);">Everything outside Allowed websites is blocked automatically. Changes sync to connected students immediately.</div>
        </div>

        <div>
          <div class="settings-section-title">Access &amp; exit passwords</div>
          <div class="settings-group">
            <div class="setting-row">
              <div>
                <div class="setting-title">Access code (entrance code)</div>
                <div class="setting-desc">${hasEntrance ? 'Active — students must enter this code before the exam loads' : 'Not required — exam opens immediately'}</div>
              </div>
              ${renderCredBadge(hasEntrance, curEntrance, 'entrance_code')}
            </div>
            <div class="add-row">
              <input type="${credEyeVisible.entrance ? 'text' : 'password'}" class="add-input" id="entrance-code-input" placeholder="${hasEntrance ? 'Enter new access code…' : 'Set access code (e.g. PERIOD2)…'}" autocomplete="off" />
              <button type="button" class="eye-btn" id="toggle-entrance-eye" aria-label="${credEyeVisible.entrance ? 'Hide access code' : 'Show access code'}" aria-pressed="${credEyeVisible.entrance ? 'true' : 'false'}" title="${credEyeVisible.entrance ? 'Hide' : 'Show'}">${eyeSvg(credEyeVisible.entrance)}</button>
              <button type="button" class="add-btn" id="set-entrance-btn">Save</button>
              ${hasEntrance ? '<button type="button" class="remove-btn" id="clear-entrance-btn" style="height:30px;padding:0 10px;">Clear</button>' : ''}
            </div>
            <div class="setting-row" style="border-top:1px solid var(--border);">
              <div>
                <div class="setting-title">Exit password (quit password)</div>
                <div class="setting-desc">${hasQuit ? 'Active — required to unlock and quit RLD Browser' : 'Not required — students can confirm to exit without a password'}</div>
              </div>
              ${renderCredBadge(hasQuit, curQuit, 'quit_password')}
            </div>
            <div class="add-row">
              <input type="${credEyeVisible.quit ? 'text' : 'password'}" class="add-input" id="quit-pwd-input" placeholder="${hasQuit ? 'Enter new exit password…' : 'Set exit password…'}" autocomplete="off" />
              <button type="button" class="eye-btn" id="toggle-quit-eye" aria-label="${credEyeVisible.quit ? 'Hide exit password' : 'Show exit password'}" aria-pressed="${credEyeVisible.quit ? 'true' : 'false'}" title="${credEyeVisible.quit ? 'Hide' : 'Show'}">${eyeSvg(credEyeVisible.quit)}</button>
              <button type="button" class="add-btn" id="set-quit-btn">Save</button>
              ${hasQuit ? '<button type="button" class="remove-btn" id="clear-quit-btn" style="height:30px;padding:0 10px;">Clear</button>' : ''}
            </div>
          </div>
        </div>

        <div>
          <div class="settings-section-title">Allowed websites (auto-opens as student tabs)</div>
          <div class="settings-group">
            ${allowedRows}
            <div class="add-row">
              <input type="text" class="add-input" id="add-allowed-input" placeholder="e.g. desmos.com, en.wikipedia.org, *.instructure.com" />
              <button type="button" class="add-btn" id="add-allowed-btn">Allow</button>
            </div>
            <div class="preset-bar">
              <button type="button" class="preset-btn" data-preset-domain="desmos.com">+ Desmos</button>
              <button type="button" class="preset-btn" data-preset-domain="en.wikipedia.org">+ Wikipedia</button>
              <button type="button" class="preset-btn" data-preset-domain="*.instructure.com">+ Canvas</button>
              <button type="button" class="preset-btn" data-preset-domain="*.collegeboard.org">+ College Board</button>
            </div>
          </div>
        </div>

        <div>
          <div class="settings-section-title">Monitoring &amp; lockdown</div>
          <div class="settings-group">
            ${row('require_webcam', 'Require webcam', 'Mandatory camera monitoring; pauses exam if student denies camera access', p.require_webcam)}
            ${row('block_clipboard', 'Block copy / paste', 'Prevent copying exam questions or pasting external text', p.block_clipboard !== false)}
            ${row('block_multiple_displays', 'Block external displays', 'Pause exam immediately if a second display or AirPlay connects', p.block_multiple_displays)}
            ${row('block_screen_capture_apps', 'Block screen recording apps', 'Flag and pause exam if OBS, ScreenSharing, or remote desktop runs', p.block_screen_capture_apps)}
            ${row('block_downloads', 'Block file downloads', 'Prevent saving files or exam PDFs to the student device', p.block_downloads)}
          </div>
        </div>

        <div>
          <div class="settings-section-title">Browser &amp; accommodations</div>
          <div class="settings-group">
            ${row('allow_read_aloud', 'Allow Read Aloud', 'Enable text-to-speech controls in students’ Accessibility menu', p.allow_read_aloud)}
            ${row('allow_find_in_page', 'Allow Find-in-Page (Cmd+F)', 'Let students search text inside the current exam page', p.allow_find_in_page)}
            ${row('allow_back_forward', 'Allow Back / Forward', 'Permit browser back and forward history navigation', p.allow_back_forward)}
            ${row('allow_reload', 'Allow page reload', 'Permit refreshing the current exam or reference page', p.allow_reload)}
            ${row('allow_zoom', 'Allow page zoom', 'Permit zooming text between 50% and 200%', p.allow_zoom)}
          </div>
        </div>
      </div>`;

      wrap.querySelectorAll('[data-policy-key]').forEach((sw) => {
        sw.addEventListener('click', () => {
          const key = sw.dataset.policyKey;
          const nextVal = sw.getAttribute('aria-checked') !== 'true';
          savePolicy({ [key]: nextVal });
        });
      });

      wrap.querySelectorAll('[data-copy-cred]').forEach((btn) => {
        btn.addEventListener('click', async () => {
          const val = latestCredentials && latestCredentials[btn.dataset.copyCred];
          if (!val) return;
          try {
            await navigator.clipboard.writeText(val);
            btn.textContent = 'Copied';
            setTimeout(() => { btn.textContent = 'Copy'; }, 1400);
          } catch (_) {}
        });
      });

      const entInput = document.getElementById('entrance-code-input');
      entInput.value = settingsDrafts.entrance;
      entInput.addEventListener('input', () => { settingsDrafts.entrance = entInput.value; });
      const entEye = document.getElementById('toggle-entrance-eye');
      entEye.addEventListener('click', () => {
        credEyeVisible.entrance = !credEyeVisible.entrance;
        entInput.type = credEyeVisible.entrance ? 'text' : 'password';
        entEye.setAttribute('aria-pressed', credEyeVisible.entrance ? 'true' : 'false');
        entEye.setAttribute('aria-label', credEyeVisible.entrance ? 'Hide access code' : 'Show access code');
        entEye.title = credEyeVisible.entrance ? 'Hide' : 'Show';
        entEye.innerHTML = eyeSvg(credEyeVisible.entrance);
      });
      document.getElementById('set-entrance-btn').addEventListener('click', () => {
        if (entInput.value.trim()) saveCredential('entrance_code', entInput.value.trim());
      });
      entInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && entInput.value.trim()) saveCredential('entrance_code', entInput.value.trim());
      });
      const clearEnt = document.getElementById('clear-entrance-btn');
      if (clearEnt) clearEnt.addEventListener('click', () => saveCredential('entrance_code', null));

      const quitInput = document.getElementById('quit-pwd-input');
      quitInput.value = settingsDrafts.quit;
      quitInput.addEventListener('input', () => { settingsDrafts.quit = quitInput.value; });
      const quitEye = document.getElementById('toggle-quit-eye');
      quitEye.addEventListener('click', () => {
        credEyeVisible.quit = !credEyeVisible.quit;
        quitInput.type = credEyeVisible.quit ? 'text' : 'password';
        quitEye.setAttribute('aria-pressed', credEyeVisible.quit ? 'true' : 'false');
        quitEye.setAttribute('aria-label', credEyeVisible.quit ? 'Hide exit password' : 'Show exit password');
        quitEye.title = credEyeVisible.quit ? 'Hide' : 'Show';
        quitEye.innerHTML = eyeSvg(credEyeVisible.quit);
      });
      document.getElementById('set-quit-btn').addEventListener('click', () => {
        if (quitInput.value.trim()) saveCredential('quit_password', quitInput.value.trim());
      });
      quitInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' && quitInput.value.trim()) saveCredential('quit_password', quitInput.value.trim());
      });
      const clearQuit = document.getElementById('clear-quit-btn');
      if (clearQuit) clearQuit.addEventListener('click', () => saveCredential('quit_password', null));

      const addDomain = (raw) => {
        const val = (raw || '').trim().toLowerCase().replace(/^https?:\/\//, '').split('/')[0];
        if (!val) return;
        settingsDrafts.allowed = '';
        const list = Array.isArray(p.allowed_domains) ? [...p.allowed_domains] : [];
        if (!list.includes(val)) list.push(val);
        savePolicy({ allowed_domains: list });
      };
      const allowedInput = document.getElementById('add-allowed-input');
      allowedInput.value = settingsDrafts.allowed;
      allowedInput.addEventListener('input', () => { settingsDrafts.allowed = allowedInput.value; });
      document.getElementById('add-allowed-btn').addEventListener('click', () => addDomain(allowedInput.value));
      allowedInput.addEventListener('keydown', (e) => { if (e.key === 'Enter') addDomain(allowedInput.value); });
      wrap.querySelectorAll('[data-preset-domain]').forEach((b) => {
        b.addEventListener('click', () => addDomain(b.dataset.presetDomain));
      });
      wrap.querySelectorAll('[data-rm-allowed]').forEach((b) => {
        b.addEventListener('click', () => {
          const idx = Number(b.dataset.rmAllowed);
          savePolicy({ allowed_domains: allowed.filter((_, i) => i !== idx) });
        });
      });

      if (prevFocusId) {
        const nextFocus = document.getElementById(prevFocusId);
        if (nextFocus) {
          nextFocus.focus();
          if (prevSelStart !== null && prevSelEnd !== null && typeof nextFocus.setSelectionRange === 'function') {
            try { nextFocus.setSelectionRange(prevSelStart, prevSelEnd); } catch (_) {}
          }
        }
      }

      renderInspector();
      return;
    }

    lastSettingsSig = '';
    const q = (document.getElementById('search-input').value || '').trim().toLowerCase();
    const list = (latestState.students || []).filter((s) => {
      if (currentView === 'cameras' && s.status === 'exited') return false;
      if (currentView === 'attention' && !isFlagged(s)) return false;
      if (q && !s.student_id.toLowerCase().includes(q) && !(s.active_host || '').toLowerCase().includes(q)) return false;
      return true;
    });

    if (list.length === 0) {
      lastCamGridSig = '';
      wrap.innerHTML = `<div class="empty">
        <div class="empty-title">${currentView === 'attention' ? 'No issues right now' : 'Waiting for students'}</div>
        <div class="empty-sub">${currentView === 'attention' ? 'All connected students pass security checks' : 'Connected exam sessions appear here automatically'}</div>
      </div>`;
      renderInspector();
      return;
    }

    if (currentView === 'cameras') {
      const camSig = JSON.stringify([tileWidth, selectedId, list.map((s) => [s.student_id, s.status, s.config_key_verified, s.violations, s.has_webcam, s.has_screen])]);
      if (camSig === lastCamGridSig) {
        // Update lightweight heartbeat ticks and active hosts in place without touching live <img> nodes.
        for (const s of list) {
          const tick = wrap.querySelector(`[data-tick="${CSS.escape(s.student_id)}"]`);
          if (tick) tick.textContent = s.last_seen_secs_ago + 's';
          const host = wrap.querySelector(`[data-host="${CSS.escape(s.student_id)}"]`);
          if (host) host.textContent = s.active_host || '';
        }
        renderInspector();
        return;
      }
      lastCamGridSig = camSig;

      const tiles = list.map((s) => {
        const [dotCls] = statusMeta(s.status);
        const camSrc = mediaUrls.get(`0:${s.student_id}`) || '';
        const screenSrc = mediaUrls.get(`1:${s.student_id}`) || '';
        const media = s.has_webcam
          ? `<img data-cam-img="${esc(s.student_id)}"${camSrc ? ` src="${camSrc}"` : ''} alt="${esc(s.student_id)}" />`
          : `<span data-cam-placeholder="${esc(s.student_id)}">Camera off</span>`;
        const flagBar = (s.violations && s.violations.length > 0)
          ? `<div class="cam-flag-bar">${esc(s.violations[0])}</div>`
          : (!s.config_key_verified ? '<div class="cam-flag-bar">Config key mismatch</div>' : '');
        const pip = s.has_screen
          ? `<div class="cam-pip${flagBar ? ' lifted' : ''}"><img data-screen-img="${esc(s.student_id)}"${screenSrc ? ` src="${screenSrc}"` : ''} alt="Student screen" /></div>`
          : '';
        const cls = ['cam-tile', isFlagged(s) ? 'flagged' : '', selectedId === s.student_id ? 'selected' : ''].join(' ');
        return `<div class="${cls}" data-select="${esc(s.student_id)}">
          <div class="cam-frame" data-cam-box="${esc(s.student_id)}">${media}${pip}${flagBar}</div>
          <div class="cam-foot">
            <div class="cam-id"><span class="dot ${dotCls}" style="margin:0;flex-shrink:0"></span><span class="cam-name">${esc(s.student_id)}</span><span class="cam-host" data-host="${esc(s.student_id)}">${esc(s.active_host || '')}</span></div>
            <span class="cam-tick" data-tick="${esc(s.student_id)}">${s.last_seen_secs_ago}s</span>
          </div>
        </div>`;
      }).join('');
      wrap.innerHTML = `<div class="cam-grid" style="--tile-w:${tileWidth}px">${tiles}</div>`;
    } else {
      lastCamGridSig = '';
      const rows = list.map((s) => {
        const [dotCls, label] = statusMeta(s.status);
        const issue = (s.violations && s.violations.length > 0)
          ? `<span class="err-text">${esc(s.violations.join(' · '))}</span>`
          : (!s.config_key_verified ? '<span class="err-text">Config key mismatch</span>' : '<span class="muted">—</span>');
        const cls = [isFlagged(s) ? 'flagged' : '', selectedId === s.student_id ? 'selected' : ''].join(' ');
        const action = s.force_unlock_pending
          ? '<span class="mono muted">Ending…</span>'
          : (s.status === 'exited'
            ? '<span class="mono muted">Ended</span>'
            : `<button type="button" class="row-action" title="Remotely exit lockdown for this student" data-unlock="${esc(s.student_id)}">End session</button>`);
        return `<tr class="${cls}" data-select="${esc(s.student_id)}">
          <td><strong>${esc(s.student_id)}</strong></td>
          <td><span class="dot ${dotCls}"></span>${esc(label)}</td>
          <td class="mono muted">${esc(s.active_host || '—')}</td>
          <td class="mono ${s.config_key_verified ? 'muted' : 'err-text'}">${s.config_key_verified ? 'Verified' : 'Mismatch'}</td>
          <td>${issue}</td>
          <td class="mono muted">${s.last_seen_secs_ago}s</td>
          <td style="text-align:right">${action}</td>
        </tr>`;
      }).join('');
      wrap.innerHTML = `<table>
        <thead><tr>
          <th>Student</th><th>Status</th><th>Host</th><th>Config key</th><th>Events</th><th>Seen</th><th style="text-align:right">Action</th>
        </tr></thead>
        <tbody>${rows}</tbody>
      </table>`;
    }

    wrap.querySelectorAll('[data-select]').forEach((el) => {
      el.addEventListener('click', (e) => {
        if (e.target.closest('[data-unlock]')) return;
        setSelectedId(el.dataset.select);
        render();
      });
    });
    wrap.querySelectorAll('[data-unlock]').forEach((btn) => {
      btn.addEventListener('click', (e) => {
        e.stopPropagation();
        unlockOne(btn.dataset.unlock);
      });
    });

    renderInspector();
  };

  const refresh = async (forceRender = false) => {
    if (!isAuthenticated) return;
    try {
      const [res, credsRes, roomsRes] = await Promise.all([
        fetch('/api/state'),
        fetch('/api/credentials'),
        fetch('/api/rooms'),
      ]);
      if (res.status === 401) {
        showAuthScreen(true);
        return;
      }
      latestState = await res.json();
      if (credsRes.ok) {
        latestCredentials = await credsRes.json();
      }
      if (roomsRes.ok) {
        latestRooms = await roomsRes.json();
      }
      const students = latestState.students || [];
      const camCount = students.filter((s) => s.status !== 'exited' && s.has_webcam).length;

      document.getElementById('room-label').textContent = latestState.room_code;
      document.getElementById('cnt-cams').textContent = camCount;
      document.getElementById('cnt-all').textContent = latestState.total_students;
      document.getElementById('cnt-rooms').textContent = Array.isArray(latestRooms.rooms) ? latestRooms.rooms.length : 0;
      const flagEl = document.getElementById('cnt-flag');
      flagEl.textContent = latestState.flagged_count;
      flagEl.classList.toggle('alert', latestState.flagged_count > 0);

      document.getElementById('sum-active').textContent = latestState.in_exam_count;
      const sumFlag = document.getElementById('sum-flagged');
      sumFlag.textContent = latestState.flagged_count;
      sumFlag.classList.toggle('alert', latestState.flagged_count > 0);
      document.getElementById('sum-total').textContent = latestState.total_students;
      document.getElementById('sum-key').textContent = latestState.expected_config_key
        ? latestState.expected_config_key.slice(0, 16) + '…'
        : 'Unpinned';

      if (forceRender) {
        lastSettingsSig = '';
        lastRoomsSig = '';
      }
      render();
    } catch (_) {}
  };

  const initAuth = async () => {
    try {
      const res = await fetch('/api/auth/me');
      const me = await res.json();
      if (me && me.authenticated) {
        enterConsole(me);
      } else {
        showAuthScreen(Boolean(me && me.has_accounts));
      }
    } catch (_) {
      showAuthScreen(false);
    }
  };

  initAuth();
</script>
</body>
</html>"##
}
