use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::ExamConfig;

const MIN_ZOOM: f64 = 0.5;
const MAX_ZOOM: f64 = 2.0;
const ZOOM_STEP: f64 = 0.1;

/// A policy-defined pinned tab that opens alongside the main exam tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedTab {
    pub title: String,
    pub url: Url,
}

/// Browser-level policy flags controlling session isolation, downloads, zoom, find, and UA.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserPolicy {
    pub ephemeral_session: bool,
    pub block_downloads: bool,
    pub allow_zoom: bool,
    pub allow_find_in_page: bool,
    pub user_agent: Option<String>,
}

impl Default for BrowserPolicy {
    fn default() -> Self {
        Self {
            ephemeral_session: true,
            block_downloads: true,
            allow_zoom: true,
            allow_find_in_page: false,
            user_agent: None,
        }
    }
}

/// Identifier for a managed browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TabId(pub usize);

/// Runtime state of a single managed browser tab.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TabState {
    pub id: TabId,
    pub title: String,
    pub url: String,
    pub host: String,
    pub is_loading: bool,
    pub pinned: bool,
}

/// Manages the policy-defined tab list, active tab selection, and shared zoom level.
#[derive(Debug, Clone)]
pub struct TabManager {
    tabs: Vec<TabState>,
    active_index: usize,
    zoom_level: f64,
    next_id: usize,
}

impl TabManager {
    /// Builds a [`TabManager`] from `config`, placing the main exam tab at index 0
    /// followed by any configured [`PinnedTab`] entries.
    pub fn from_config(config: &ExamConfig) -> Self {
        let mut tabs = Vec::with_capacity(1 + config.pinned_tabs.len());
        tabs.push(TabState {
            id: TabId(0),
            title: config.exam_name.clone(),
            url: config.start_url.to_string(),
            host: config.start_url.host_str().unwrap_or_default().to_string(),
            is_loading: false,
            pinned: false,
        });

        for (idx, pinned) in config.pinned_tabs.iter().enumerate() {
            tabs.push(TabState {
                id: TabId(idx + 1),
                title: pinned.title.clone(),
                url: pinned.url.to_string(),
                host: pinned.url.host_str().unwrap_or_default().to_string(),
                is_loading: false,
                pinned: true,
            });
        }

        let next_id = tabs.len();
        Self {
            tabs,
            active_index: 0,
            zoom_level: 1.0,
            next_id,
        }
    }

    pub fn tabs(&self) -> &[TabState] {
        &self.tabs
    }

    pub fn active_id(&self) -> TabId {
        self.tabs[self.active_index].id
    }

    pub fn active_tab(&self) -> &TabState {
        &self.tabs[self.active_index]
    }

    pub fn has_multiple_tabs(&self) -> bool {
        self.tabs.len() > 1
    }

    /// Activates the tab with `id`, returning `true` if it exists.
    pub fn activate(&mut self, id: TabId) -> bool {
        if let Some(pos) = self.tabs.iter().position(|tab| tab.id == id) {
            self.active_index = pos;
            true
        } else {
            false
        }
    }

    /// Reorders the tab `from_id` to the position currently occupied by `to_id`,
    /// preserving each tab's stable `TabId` and keeping the active tab selected.
    pub fn reorder_by_id(&mut self, from_id: TabId, to_id: TabId) -> bool {
        if from_id == to_id {
            return false;
        }
        let Some(from_pos) = self.tabs.iter().position(|tab| tab.id == from_id) else {
            return false;
        };
        let Some(to_pos) = self.tabs.iter().position(|tab| tab.id == to_id) else {
            return false;
        };
        let active_id = self.active_id();
        let moved = self.tabs.remove(from_pos);
        self.tabs.insert(to_pos, moved);
        if let Some(new_active) = self.tabs.iter().position(|tab| tab.id == active_id) {
            self.active_index = new_active;
        }
        true
    }

    /// Reorders tabs to match `ordered_ids` (ignoring unknown IDs and keeping any unlisted tabs at the end).
    pub fn set_order(&mut self, ordered_ids: &[TabId]) -> bool {
        let active_id = self.active_id();
        let mut new_tabs = Vec::with_capacity(self.tabs.len());
        let mut remaining = self.tabs.clone();
        for &id in ordered_ids {
            if let Some(pos) = remaining.iter().position(|t| t.id == id) {
                new_tabs.push(remaining.remove(pos));
            }
        }
        new_tabs.extend(remaining);
        let changed = new_tabs.iter().map(|t| t.id).ne(self.tabs.iter().map(|t| t.id));
        self.tabs = new_tabs;
        if let Some(new_active) = self.tabs.iter().position(|tab| tab.id == active_id) {
            self.active_index = new_active;
        }
        changed
    }

    /// Appends a new managed tab and returns its assigned [`TabId`].
    pub fn add_tab(&mut self, title: String, url: Url, pinned: bool) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        let host = url.host_str().unwrap_or_default().to_string();
        self.tabs.push(TabState {
            id,
            title,
            url: url.to_string(),
            host,
            is_loading: false,
            pinned,
        });
        id
    }

    /// Removes a non-primary tab by `id`, preserving active selection if possible.
    pub fn remove_tab(&mut self, id: TabId) -> bool {
        if id == TabId(0) {
            return false;
        }
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else {
            return false;
        };
        let active_id = self.active_id();
        self.tabs.remove(pos);
        if let Some(new_active) = self.tabs.iter().position(|t| t.id == active_id) {
            self.active_index = new_active;
        } else {
            self.active_index = pos.saturating_sub(1).min(self.tabs.len().saturating_sub(1));
        }
        true
    }

    pub fn set_title(&mut self, id: TabId, title: String) {
        if let Some(tab) = self.tab_mut(id) {
            tab.title = title;
        }
    }

    pub fn set_host(&mut self, id: TabId, host: String) {
        if let Some(tab) = self.tab_mut(id) {
            tab.host = host;
        }
    }

    pub fn set_loading(&mut self, id: TabId, loading: bool) {
        if let Some(tab) = self.tab_mut(id) {
            tab.is_loading = loading;
        }
    }

    pub fn zoom_level(&self) -> f64 {
        self.zoom_level
    }

    pub fn zoom_percent(&self) -> u32 {
        (self.zoom_level * 100.0).round() as u32
    }

    pub fn zoom_in(&mut self) -> f64 {
        self.zoom_level = clamp_and_round_zoom(self.zoom_level + ZOOM_STEP);
        self.zoom_level
    }

    pub fn zoom_out(&mut self) -> f64 {
        self.zoom_level = clamp_and_round_zoom(self.zoom_level - ZOOM_STEP);
        self.zoom_level
    }

    pub fn zoom_reset(&mut self) -> f64 {
        self.zoom_level = 1.0;
        self.zoom_level
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut TabState> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }
}

fn clamp_and_round_zoom(value: f64) -> f64 {
    (value.clamp(MIN_ZOOM, MAX_ZOOM) * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LockdownPolicy;
    use crate::security::SecurityPolicy;

    fn config_with_pinned(pinned_tabs: Vec<PinnedTab>) -> ExamConfig {
        ExamConfig {
            exam_name: "Final Exam".to_string(),
            start_url: Url::parse("https://exam.school.edu/start").unwrap(),
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            entrance_code_hash: None,
            quit_password_hash: None,
            lockdown: LockdownPolicy::default(),
            security: SecurityPolicy::default(),
            pinned_tabs,
            browser: BrowserPolicy::default(),
            accessibility: crate::config::AccessibilityPolicy::default(),
            proctor: crate::proctor::ProctorPolicy::default(),
        }
    }

    #[test]
    fn browser_policy_defaults_are_strict() {
        let policy = BrowserPolicy::default();
        assert!(policy.ephemeral_session);
        assert!(policy.block_downloads);
        assert!(policy.allow_zoom);
        assert!(!policy.allow_find_in_page);
        assert_eq!(policy.user_agent, None);
    }

    #[test]
    fn tab_manager_initializes_from_config_and_activates_tabs() {
        let cfg = config_with_pinned(vec![
            PinnedTab {
                title: "Formula Sheet".to_string(),
                url: Url::parse("https://docs.school.edu/formulas.pdf").unwrap(),
            },
            PinnedTab {
                title: "Calculator".to_string(),
                url: Url::parse("https://calc.school.edu/").unwrap(),
            },
        ]);

        let mut mgr = TabManager::from_config(&cfg);
        assert!(mgr.has_multiple_tabs());
        assert_eq!(mgr.tabs().len(), 3);
        assert_eq!(mgr.active_id(), TabId(0));
        assert_eq!(mgr.active_tab().title, "Final Exam");
        assert_eq!(mgr.active_tab().host, "exam.school.edu");
        assert!(!mgr.active_tab().pinned);

        assert!(mgr.activate(TabId(2)));
        assert_eq!(mgr.active_id(), TabId(2));
        assert_eq!(mgr.active_tab().title, "Calculator");
        assert!(mgr.active_tab().pinned);

        assert!(!mgr.activate(TabId(99)));
        assert_eq!(mgr.active_id(), TabId(2));
    }

    #[test]
    fn tab_manager_single_tab_and_state_mutations() {
        let cfg = config_with_pinned(Vec::new());
        let mut mgr = TabManager::from_config(&cfg);
        assert!(!mgr.has_multiple_tabs());
        assert_eq!(mgr.tabs().len(), 1);

        mgr.set_title(TabId(0), "Updated Title".to_string());
        mgr.set_host(TabId(0), "portal.school.edu".to_string());
        mgr.set_loading(TabId(0), true);

        let tab = mgr.active_tab();
        assert_eq!(tab.title, "Updated Title");
        assert_eq!(tab.host, "portal.school.edu");
        assert!(tab.is_loading);

        // Mutating a nonexistent tab is a safe no-op.
        mgr.set_loading(TabId(42), true);
    }

    #[test]
    fn zoom_clamps_and_rounds_without_float_drift() {
        let cfg = config_with_pinned(Vec::new());
        let mut mgr = TabManager::from_config(&cfg);

        assert_eq!(mgr.zoom_level(), 1.0);
        assert_eq!(mgr.zoom_percent(), 100);

        for _ in 0..20 {
            mgr.zoom_in();
        }
        assert_eq!(mgr.zoom_level(), 2.0);
        assert_eq!(mgr.zoom_percent(), 200);

        for _ in 0..30 {
            mgr.zoom_out();
        }
        assert_eq!(mgr.zoom_level(), 0.5);
        assert_eq!(mgr.zoom_percent(), 50);

        assert_eq!(mgr.zoom_reset(), 1.0);
        assert_eq!(mgr.zoom_percent(), 100);

        // Step across 1.1 -> 1.2 -> 1.3 -> 1.2 without IEEE-754 drift.
        assert_eq!(mgr.zoom_in(), 1.1);
        assert_eq!(mgr.zoom_in(), 1.2);
        assert_eq!(mgr.zoom_in(), 1.3);
        assert_eq!(mgr.zoom_out(), 1.2);
        assert_eq!(mgr.zoom_percent(), 120);
    }

    #[test]
    fn reorder_by_id_preserves_ids_and_active_tab() {
        let cfg = config_with_pinned(vec![
            PinnedTab {
                title: "Formulas".to_string(),
                url: Url::parse("https://docs.school.edu/formulas").unwrap(),
            },
            PinnedTab {
                title: "Desmos".to_string(),
                url: Url::parse("https://www.desmos.com/calculator").unwrap(),
            },
        ]);
        let mut mgr = TabManager::from_config(&cfg);
        assert!(mgr.activate(TabId(1)));
        assert!(mgr.reorder_by_id(TabId(2), TabId(0)));
        let ids: Vec<usize> = mgr.tabs().iter().map(|t| t.id.0).collect();
        assert_eq!(ids, vec![2, 0, 1]);
        assert_eq!(mgr.active_id(), TabId(1));
        assert!(!mgr.reorder_by_id(TabId(1), TabId(1)));
        assert!(!mgr.reorder_by_id(TabId(99), TabId(0)));
    }
}
