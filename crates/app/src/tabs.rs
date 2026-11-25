use serde::Serialize;

/// Serializable view of a single tab rendered in the HUD tab strip.
#[derive(Debug, Clone, Serialize)]
pub struct TabView {
    pub id: usize,
    pub title: String,
    pub host: String,
    pub is_loading: bool,
    pub pinned: bool,
}

/// Snapshot of tab strip and zoom state sent from Rust to `window.__hud.syncTabs(...)`.
#[derive(Debug, Clone, Serialize)]
pub struct TabStripSnapshot {
    pub tabs: Vec<TabView>,
    pub active_id: usize,
    pub zoom_percent: u32,
}

/// Builds a self-contained Quiet Native HTML recovery page for blocked navigations.
#[allow(dead_code)]
pub fn build_blocked_page_html(url: &str, reason: &str) -> String {
    let escaped_url = escape_html(url);
    let escaped_reason = escape_html(reason);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Page Unavailable</title>
<style>
  * {{
    box-sizing: border-box;
    margin: 0;
    padding: 0;
    -webkit-font-smoothing: antialiased;
  }}
  html, body {{
    width: 100%;
    height: 100%;
    font-family: "SF Pro", -apple-system, BlinkMacSystemFont, "Helvetica Neue", system-ui, sans-serif;
    background: #FFFFFF;
    color: #192230;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
  }}
  .panel {{
    width: 100%;
    max-width: 480px;
  }}
  h1 {{
    font-size: 25px;
    font-weight: 590;
    letter-spacing: -0.012em;
    line-height: 32px;
    color: #192230;
    margin-bottom: 11px;
  }}
  .reason {{
    font-size: 14px;
    line-height: 21px;
    color: #4A5566;
    margin-bottom: 10px;
  }}
  .url {{
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    line-height: 18px;
    color: #8994A4;
    word-break: break-all;
    margin-bottom: 24px;
  }}
  .btn {{
    appearance: none;
    height: 32px;
    padding: 0 16px;
    border-radius: 7px;
    border: 1px solid #235DBC;
    background: #235DBC;
    color: #FFFFFF;
    font-family: inherit;
    font-size: 12px;
    font-weight: 590;
    cursor: pointer;
  }}
  .btn:hover {{
    background: #1D4FA0;
    border-color: #1D4FA0;
  }}
</style>
</head>
<body>
  <main class="panel" role="main">
    <h1>This page isn&rsquo;t available during the exam</h1>
    <p class="reason">{escaped_reason}</p>
    <p class="url">{escaped_url}</p>
    <button type="button" class="btn" onclick="history.back()">Return to exam</button>
  </main>
</body>
</html>"#
    )
}

#[allow(dead_code)]
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

    #[test]
    fn blocked_page_escapes_html_in_url_and_reason() {
        let html = build_blocked_page_html(
            "https://evil.example/?q=<script>alert('x')</script>&a=1",
            "host \"evil.example\" <not allowed> & blocked",
        );

        assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;&amp;a=1"));
        assert!(html.contains("host &quot;evil.example&quot; &lt;not allowed&gt; &amp; blocked"));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("Return to exam"));
    }

    #[test]
    fn tab_strip_snapshot_serializes_expected_json() {
        let snapshot = TabStripSnapshot {
            tabs: vec![
                TabView {
                    id: 0,
                    title: "Midterm Exam".to_string(),
                    host: "exam.school.edu".to_string(),
                    is_loading: false,
                    pinned: false,
                },
                TabView {
                    id: 1,
                    title: "Formula Sheet".to_string(),
                    host: "docs.school.edu".to_string(),
                    is_loading: true,
                    pinned: true,
                },
            ],
            active_id: 0,
            zoom_percent: 110,
        };

        let value = serde_json::to_value(&snapshot).expect("serialize TabStripSnapshot");
        assert_eq!(value["active_id"], 0);
        assert_eq!(value["zoom_percent"], 110);
        assert_eq!(value["tabs"][0]["title"], "Midterm Exam");
        assert_eq!(value["tabs"][1]["is_loading"], true);
        assert_eq!(value["tabs"][1]["pinned"], true);
    }
}
