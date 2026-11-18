//! The window layout: three panels (Browser, Inspector, Dock) that can be shown, hidden, collapsed to a rail and moved to a side,
//! the built-in layouts (Simple, Editing, Review) and the user's saved ones. This module is pure state; `app_r.rs` draws it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserTab {
    #[default]
    Maps,
    Outliner,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockTab {
    #[default]
    Problems,
    Changes,
    Find,
    Spawns,
}

impl DockTab {
    pub const ALL: [DockTab; 4] = [DockTab::Problems, DockTab::Changes, DockTab::Find, DockTab::Spawns];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelId {
    Browser,
    Inspector,
    Dock,
}

impl PanelId {
    pub const ALL: [PanelId; 3] = [PanelId::Browser, PanelId::Inspector, PanelId::Dock];

    pub fn label(self) -> &'static str {
        match self {
            PanelId::Browser => "Browser",
            PanelId::Inspector => "Inspector",
            PanelId::Dock => "Dock",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelState {
    pub visible: bool,
    /// Drawn as a thin rail with its tab icons instead of the full panel.
    pub collapsed: bool,
    pub side: Side,
    /// Width of a side panel, height of a bottom one.
    pub size: f32,
}

impl Default for PanelState {
    fn default() -> Self {
        PanelState { visible: true, collapsed: false, side: Side::Left, size: 240.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutState {
    pub browser: PanelState,
    pub inspector: PanelState,
    pub dock: PanelState,
    pub browser_tab: BrowserTab,
    /// Maps above the Outliner in one Browser (the older layout).
    pub browser_split: bool,
    pub dock_tab: DockTab,
    /// Share of the height the Browser gets when it is stacked above the Inspector.
    pub stack_ratio: f32,
}

pub const DEFAULT_STACK_RATIO: f32 = 0.45;
pub const SPLIT_RATIO: f32 = 0.36;

impl Default for LayoutState {
    fn default() -> Self {
        LayoutState::simple()
    }
}

/// The built-in layout names, in menu order.
pub const BUILTIN: [&str; 3] = ["Simple", "Editing", "Review"];

impl LayoutState {
    /// Browser left (Maps), Inspector right, Dock hidden.
    pub fn simple() -> LayoutState {
        LayoutState {
            browser: PanelState { visible: true, collapsed: false, side: Side::Left, size: 240.0 },
            inspector: PanelState { visible: true, collapsed: false, side: Side::Right, size: 320.0 },
            dock: PanelState { visible: false, collapsed: false, side: Side::Bottom, size: 170.0 },
            browser_tab: BrowserTab::Maps,
            browser_split: false,
            dock_tab: DockTab::Problems,
            stack_ratio: DEFAULT_STACK_RATIO,
        }
    }

    /// Outliner on the left, Dock collapsed to a rail at the bottom.
    pub fn editing() -> LayoutState {
        let mut l = LayoutState::simple();
        l.browser_tab = BrowserTab::Outliner;
        l.dock.visible = true;
        l.dock.collapsed = true;
        l
    }

    /// Dock open on Problems.
    pub fn review() -> LayoutState {
        let mut l = LayoutState::simple();
        l.dock.visible = true;
        l.dock.size = 240.0;
        l.dock_tab = DockTab::Problems;
        l
    }

    pub fn builtin(name: &str) -> Option<LayoutState> {
        match name {
            "Simple" => Some(LayoutState::simple()),
            "Editing" => Some(LayoutState::editing()),
            "Review" => Some(LayoutState::review()),
            _ => None,
        }
    }

    /// A settings file from before the layouts: Simple with the old panel sizes.
    pub fn migrated(left: f32, right: f32, bottom: f32) -> LayoutState {
        let mut l = LayoutState::simple();
        let ok = |v: f32, d: f32| if v.is_finite() && v > 50.0 { v } else { d };
        l.browser.size = ok(left, l.browser.size);
        l.inspector.size = ok(right, l.inspector.size);
        l.dock.size = ok(bottom, l.dock.size);
        l
    }

    pub fn panel(&self, p: PanelId) -> &PanelState {
        match p {
            PanelId::Browser => &self.browser,
            PanelId::Inspector => &self.inspector,
            PanelId::Dock => &self.dock,
        }
    }

    pub fn panel_mut(&mut self, p: PanelId) -> &mut PanelState {
        match p {
            PanelId::Browser => &mut self.browser,
            PanelId::Inspector => &mut self.inspector,
            PanelId::Dock => &mut self.dock,
        }
    }

    /// Browser and Inspector are on a side; the Dock is bottom or right. A hand-edited file cannot put a panel where it does not go.
    pub fn sanitised(mut self) -> LayoutState {
        for p in [PanelId::Browser, PanelId::Inspector] {
            let s = self.panel_mut(p);
            if s.side == Side::Bottom {
                s.side = Side::Left;
            }
        }
        if self.dock.side == Side::Left {
            self.dock.side = Side::Bottom;
        }
        for p in PanelId::ALL {
            let s = self.panel_mut(p);
            if !s.size.is_finite() || s.size < 60.0 {
                s.size = 240.0;
            }
        }
        if !self.stack_ratio.is_finite() {
            self.stack_ratio = DEFAULT_STACK_RATIO;
        }
        self.stack_ratio = self.stack_ratio.clamp(0.15, 0.85);
        self
    }

    pub fn show(&mut self, p: PanelId) {
        let s = self.panel_mut(p);
        s.visible = true;
        s.collapsed = false;
    }

    pub fn hide(&mut self, p: PanelId) {
        self.panel_mut(p).visible = false;
    }

    /// Ctrl+1 / 2 / 3: hidden or collapsed becomes open, open becomes hidden.
    pub fn toggle(&mut self, p: PanelId) {
        let s = self.panel(p);
        if s.visible && !s.collapsed {
            self.hide(p);
        } else {
            self.show(p);
        }
    }

    pub fn is_open(&self, p: PanelId) -> bool {
        let s = self.panel(p);
        s.visible && !s.collapsed
    }

    pub fn set_collapsed(&mut self, p: PanelId, c: bool) {
        self.panel_mut(p).collapsed = c;
    }

    /// The other side: Browser and Inspector left and right, the Dock bottom and right.
    pub fn move_side(&mut self, p: PanelId) {
        let s = self.panel_mut(p);
        s.side = match (p, s.side) {
            (PanelId::Dock, Side::Bottom) => Side::Right,
            (PanelId::Dock, _) => Side::Bottom,
            (_, Side::Left) => Side::Right,
            (_, _) => Side::Left,
        };
    }

    /// Shows the Dock (open, not collapsed) on a tab.
    pub fn open_dock(&mut self, tab: DockTab) {
        self.dock_tab = tab;
        self.show(PanelId::Dock);
    }

    /// The Dock shows this tab right now (visible and not collapsed).
    pub fn dock_showing(&self, tab: DockTab) -> bool {
        self.is_open(PanelId::Dock) && self.dock_tab == tab
    }

    /// The expanded Browser and Inspector panels on a side, top to bottom.
    pub fn stacked(&self, side: Side) -> Vec<PanelId> {
        [PanelId::Browser, PanelId::Inspector].into_iter().filter(|p| self.is_open(*p) && self.panel(*p).side == side).collect()
    }

    /// The collapsed panels (rail tabs) on a side.
    pub fn rails(&self, side: Side) -> Vec<PanelId> {
        PanelId::ALL.into_iter().filter(|p| {
            let s = self.panel(*p);
            s.visible && s.collapsed && s.side == side
        }).collect()
    }

    /// Whether two layouts arrange the panels the same way. Sizes are ignored: dragging a border is not a change of layout.
    pub fn same_arrangement(&self, o: &LayoutState) -> bool {
        PanelId::ALL.iter().all(|p| {
            let (a, b) = (self.panel(*p), o.panel(*p));
            a.visible == b.visible && a.collapsed == b.collapsed && a.side == b.side
        }) && self.browser_tab == o.browser_tab
            && self.browser_split == o.browser_split
            && self.dock_tab == o.dock_tab
    }
}

/// A saved layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamedLayout {
    pub name: String,
    pub state: LayoutState,
}

/// The state of a layout by name: a built-in one, else a saved one.
pub fn find(name: &str, user: &[NamedLayout]) -> Option<LayoutState> {
    LayoutState::builtin(name).or_else(|| user.iter().find(|l| l.name == name).map(|l| l.state.clone()))
}

/// Checks a name typed in Save layout... (trimmed): built-in names are refused.
pub fn check_name(name: &str) -> Result<String, &'static str> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Type a name.");
    }
    if BUILTIN.iter().any(|b| b.eq_ignore_ascii_case(n)) {
        return Err("Pick another name.");
    }
    Ok(n.to_string())
}

/// Saves `state` under `name` (replaces a layout of that name).
pub fn save(user: &mut Vec<NamedLayout>, name: &str, state: LayoutState) -> Result<(), &'static str> {
    let n = check_name(name)?;
    match user.iter_mut().find(|l| l.name == n) {
        Some(l) => l.state = state,
        None => user.push(NamedLayout { name: n, state }),
    }
    Ok(())
}

/// Viewport only (F11): the layout to restore, kept in memory.
#[derive(Default)]
pub struct ViewportOnly {
    stash: Option<LayoutState>,
}

impl ViewportOnly {
    pub fn active(&self) -> bool {
        self.stash.is_some()
    }

    /// F11: hides every panel (remembering the layout) or restores it exactly.
    pub fn toggle(&mut self, current: &mut LayoutState) {
        match self.stash.take() {
            Some(old) => *current = old,
            None => {
                self.stash = Some(current.clone());
                for p in PanelId::ALL {
                    current.hide(p);
                }
            }
        }
    }

    /// The stored layout (what a settings write keeps while the panels are hidden).
    pub fn stored(&self) -> Option<&LayoutState> {
        self.stash.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_layouts_match_the_spec() {
        let s = LayoutState::simple();
        assert!(s.browser.visible && s.inspector.visible && !s.dock.visible);
        assert_eq!((s.browser.side, s.inspector.side, s.dock.side), (Side::Left, Side::Right, Side::Bottom));
        assert_eq!((s.browser_tab, s.browser.size, s.inspector.size), (BrowserTab::Maps, 240.0, 320.0));
        let e = LayoutState::editing();
        assert_eq!(e.browser_tab, BrowserTab::Outliner);
        assert!(e.dock.visible && e.dock.collapsed);
        let r = LayoutState::review();
        assert!(r.dock.visible && !r.dock.collapsed && r.dock_tab == DockTab::Problems && r.dock.size == 240.0);
        assert_eq!(LayoutState::default(), s);
        for n in BUILTIN {
            assert!(LayoutState::builtin(n).is_some());
        }
        assert!(LayoutState::builtin("Mine").is_none());
    }

    #[test]
    fn panels_toggle_collapse_and_move() {
        let mut l = LayoutState::simple();
        l.toggle(PanelId::Dock);
        assert!(l.is_open(PanelId::Dock));
        l.toggle(PanelId::Dock);
        assert!(!l.dock.visible);
        l.set_collapsed(PanelId::Inspector, true);
        assert_eq!(l.rails(Side::Right), vec![PanelId::Inspector]);
        assert!(l.stacked(Side::Right).is_empty());
        l.toggle(PanelId::Inspector);
        assert!(l.is_open(PanelId::Inspector), "a collapsed panel opens first");
        l.move_side(PanelId::Browser);
        assert_eq!(l.stacked(Side::Right), vec![PanelId::Browser, PanelId::Inspector]);
        assert!(l.stacked(Side::Left).is_empty());
        l.move_side(PanelId::Dock);
        assert_eq!(l.dock.side, Side::Right);
        l.move_side(PanelId::Dock);
        assert_eq!(l.dock.side, Side::Bottom);
        l.open_dock(DockTab::Find);
        assert!(l.dock_showing(DockTab::Find) && !l.dock_showing(DockTab::Problems));
    }

    #[test]
    fn viewport_only_restores_exactly() {
        let mut l = LayoutState::review();
        l.browser.size = 313.0;
        let before = l.clone();
        let mut v = ViewportOnly::default();
        v.toggle(&mut l);
        assert!(v.active() && PanelId::ALL.iter().all(|p| !l.panel(*p).visible));
        v.toggle(&mut l);
        assert!(!v.active());
        assert_eq!(l, before);
    }

    #[test]
    fn names_and_saved_layouts() {
        assert_eq!(check_name("Simple"), Err("Pick another name."));
        assert_eq!(check_name("  review "), Err("Pick another name."));
        assert_eq!(check_name("  "), Err("Type a name."));
        assert_eq!(check_name(" Mine "), Ok("Mine".to_string()));
        let mut user = Vec::new();
        save(&mut user, "Mine", LayoutState::editing()).unwrap();
        save(&mut user, "Mine", LayoutState::review()).unwrap();
        assert_eq!(user.len(), 1);
        assert_eq!(find("Mine", &user), Some(LayoutState::review()));
        assert_eq!(find("Simple", &user), Some(LayoutState::simple()));
        assert!(find("Other", &user).is_none());
    }

    #[test]
    fn migration_keeps_the_old_sizes_and_a_bad_file_is_repaired() {
        let l = LayoutState::migrated(260.0, 330.0, 170.0);
        assert_eq!((l.browser.size, l.inspector.size, l.dock.size), (260.0, 330.0, 170.0));
        assert!(!l.dock.visible);
        let bad = LayoutState { browser: PanelState { side: Side::Bottom, size: f32::NAN, ..Default::default() }, dock: PanelState { side: Side::Left, ..Default::default() }, stack_ratio: 9.0, ..LayoutState::simple() }.sanitised();
        assert_eq!((bad.browser.side, bad.dock.side), (Side::Left, Side::Bottom));
        assert!(bad.browser.size >= 60.0 && bad.stack_ratio <= 0.85);
    }

    #[test]
    fn changed_marks_arrangement_not_border_drags() {
        let a = LayoutState::simple();
        let mut b = a.clone();
        b.browser.size = 300.0;
        assert!(a.same_arrangement(&b));
        b.browser.collapsed = true;
        assert!(!a.same_arrangement(&b));
    }
}
