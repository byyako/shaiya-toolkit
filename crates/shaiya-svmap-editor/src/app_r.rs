//! the panel layout (Browser, Inspector, Dock) with rails, sides and stacking, the tool palette, the one-row
//! toolbar, the status bar, the layouts and the menus. Declared from `app.rs` with `#[path]` so it shares the window's state.

use std::time::Duration;

use egui::containers::menu::{MenuButton, MenuConfig};
use egui::{Align, Color32, Frame, Layout, Margin, PopupCloseBehavior, Rect, RichText, Sense, Vec2};
use shaiya_svmap::SectionId;

use super::{layer_flags, position_text, App, Slot};
use crate::commands::{self, Cmd, CmdInfo, MenuEntry};
use crate::help_text;
use crate::icons::{self, Icon};
use crate::layout::{self, BrowserTab, DockTab, LayoutState, PanelId, Side};
use crate::names::Lookups;
use crate::theme;
use crate::view2d::{DrawTarget, Tool};

/// Width of the thin rail a collapsed panel leaves.
pub const RAIL: f32 = 28.0;
/// Width of the tool palette.
pub const PALETTE_W: f32 = 36.0;

/// What the toolbar of this frame drew (the tests look at it).
#[derive(Default)]
pub struct R1 {
    pub vo: layout::ViewportOnly,
    /// Save layout...: the name being typed and the reason it was refused.
    pub save_prompt: Option<(String, Option<&'static str>)>,
    /// Manage layouts is open.
    pub manage: bool,
    /// Manage layouts: (old name, text being typed) while a layout is renamed.
    pub rename: Option<(String, String)>,
    pub drawn_toolbar: Option<Rect>,
    pub drawn_status: bool,
    pub drawn_palette: Option<usize>,
    pub toolbar_overflow: bool,
    /// The rails drawn this frame: (side, width or height).
    pub rails: Vec<(Side, Rect)>,
    /// The problems chip of the toolbar (the tests click it).
    pub chip_rect: Option<Rect>,
}

/// What the header buttons of a panel asked for.
enum Hdr {
    Collapse,
    Move,
    Close,
    ResetSize,
}

fn tool_icon(t: Tool) -> Icon {
    match t {
        Tool::Select => Icon::Select,
        Tool::Box => Icon::Box,
        Tool::Npc => Icon::Npc,
        Tool::Portal => Icon::Portal,
        Tool::Paint => Icon::Paint,
        Tool::Measure => Icon::Measure,
        Tool::Scatter => Icon::Scatter,
        Tool::Place => Icon::Place,
        Tool::Terrain => Icon::Terrain,
    }
}

fn tool_key(t: Tool) -> &'static str {
    match t {
        Tool::Select => "1",
        Tool::Box => "2",
        Tool::Npc => "3",
        Tool::Portal => "4",
        Tool::Paint => "5",
        Tool::Measure => "6",
        Tool::Scatter => "7",
        Tool::Place => "8",
        Tool::Terrain => "9",
    }
}

impl App {
    // ------------------------------------------------------------------------------------------------------------ the Dock on demand

    /// Opens the Dock on a tab (shows it when hidden, expands it when collapsed). Nothing closes it again but the user.
    pub(super) fn open_dock(&mut self, tab: DockTab) {
        self.settings.layout.open_dock(tab);
    }

    /// The Dock is open on this tab.
    pub(super) fn dock_showing(&self, tab: DockTab) -> bool {
        self.settings.layout.dock_showing(tab)
    }

    // ------------------------------------------------------------------------------------------------------------ layouts

    /// Switches to a built-in or saved layout (the panels forget their remembered sizes).
    pub(super) fn apply_layout(&mut self, name: &str) {
        let Some(state) = layout::find(name, &self.settings.layouts) else { return };
        self.r1.vo = layout::ViewportOnly::default();
        self.settings.layout = state;
        self.settings.layout_name = name.to_string();
        self.layout_gen += 1;
    }

    /// The panels differ from the layout of that name (a panel was opened, closed, moved or switched since).
    pub(super) fn layout_changed(&self) -> bool {
        layout::find(&self.settings.layout_name, &self.settings.layouts).is_some_and(|l| !l.same_arrangement(&self.settings.layout))
    }

    /// The commands of the View > Panels and View > Layout menus.
    pub(super) fn run_layout_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::ShowBrowser => self.settings.layout.toggle(PanelId::Browser),
            Cmd::ShowInspector => self.settings.layout.toggle(PanelId::Inspector),
            Cmd::ShowDock => self.settings.layout.toggle(PanelId::Dock),
            Cmd::ViewportOnly => {
                let l = &mut self.settings.layout;
                self.r1.vo.toggle(l);
            }
            Cmd::LayoutSimple => self.apply_layout("Simple"),
            Cmd::LayoutEditing => self.apply_layout("Editing"),
            Cmd::LayoutReview => self.apply_layout("Review"),
            Cmd::SaveLayout => self.r1.save_prompt = Some((String::new(), None)),
            Cmd::ManageLayouts => self.r1.manage = true,
            _ => {}
        }
    }

    /// Keeps the panels as they are now under a name (a built-in name is refused).
    pub(super) fn save_layout_named(&mut self, name: &str) -> Result<(), &'static str> {
        let state = self.settings.layout.clone();
        layout::save(&mut self.settings.layouts, name, state)?;
        self.settings.layout_name = name.trim().to_string();
        self.toast(format!("layout \"{}\" saved", name.trim()));
        Ok(())
    }

    /// Manage layouts > Delete. When it is the layout in use, Simple takes over (so the menu does not show "Simple (changed)").
    pub(super) fn delete_layout(&mut self, name: &str) {
        self.settings.layouts.retain(|l| l.name != name);
        if self.settings.layout_name == name {
            self.apply_layout("Simple");
            self.toast(format!("layout \"{name}\" deleted. Simple is in use."));
        }
    }

    /// Save layout... and Manage layouts... windows.
    pub(super) fn layout_windows(&mut self, ctx: &egui::Context) {
        if let Some((mut name, mut why)) = self.r1.save_prompt.take() {
            let mut done: Option<bool> = None;
            egui::Window::new("Save layout").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.label("Name");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(240.0));
                if r.changed() {
                    why = None;
                }
                if !r.has_focus() && name.is_empty() && why.is_none() {
                    r.request_focus();
                }
                if let Some(w) = why {
                    ui.colored_label(theme::pal(ui.ctx()).error, w);
                }
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        done = Some(false);
                    }
                });
            });
            match done {
                Some(true) => {
                    match self.save_layout_named(&name) {
                        Ok(()) => {}
                        Err(w) => self.r1.save_prompt = Some((name, Some(w))),
                    }
                }
                Some(false) => {}
                None => self.r1.save_prompt = Some((name, why)),
            }
        }
        if self.r1.manage {
            let mut open = true;
            let mut delete: Option<String> = None;
            let mut rename_done: Option<(String, String)> = None;
            let mut start_rename: Option<(String, String)> = None;
            let mut cancel_rename = false;
            let rename = self.r1.rename.clone();
            let mut typing = rename.as_ref().map(|r| r.1.clone());
            egui::Window::new("Manage layouts").open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                for b in layout::BUILTIN {
                    ui.label(RichText::new(format!("{b} (built in)")).weak());
                }
                ui.separator();
                if self.settings.layouts.is_empty() {
                    ui.label("No saved layouts. View > Layout > Save layout... adds one.");
                }
                for l in &self.settings.layouts {
                    ui.horizontal(|ui| {
                        match (&rename, typing.as_mut()) {
                            (Some((old, _)), Some(text)) if *old == l.name => {
                                let r = ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
                                if ui.button("OK").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                                    rename_done = Some((old.clone(), text.clone()));
                                }
                                if ui.button("Cancel").clicked() {
                                    cancel_rename = true;
                                }
                            }
                            _ => {
                                ui.label(&l.name);
                                if ui.button("Rename").clicked() {
                                    start_rename = Some((l.name.clone(), l.name.clone()));
                                }
                                if ui.button("Delete").clicked() {
                                    delete = Some(l.name.clone());
                                }
                            }
                        }
                    });
                }
            });
            if let (Some(r), Some(t)) = (self.r1.rename.as_mut(), typing) {
                r.1 = t;
            }
            if let Some(s) = start_rename {
                self.r1.rename = Some(s);
            }
            if cancel_rename {
                self.r1.rename = None;
            }
            if let Some(n) = delete {
                self.delete_layout(&n);
            }
            if let Some((old, new)) = rename_done {
                match layout::check_name(&new) {
                    Ok(n) if n == old => self.r1.rename = None,
                    Ok(n) if self.settings.layouts.iter().any(|l| l.name == n) => self.toast("A layout with that name exists."),
                    Ok(n) => {
                        if let Some(l) = self.settings.layouts.iter_mut().find(|l| l.name == old) {
                            l.name = n.clone();
                        }
                        if self.settings.layout_name == old {
                            self.settings.layout_name = n;
                        }
                        self.r1.rename = None;
                    }
                    Err(w) => self.toast(w),
                }
            }
            if !open {
                self.r1.manage = false;
                self.r1.rename = None;
            }
        }
    }

    // ------------------------------------------------------------------------------------------------------------ menus

    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            for top in commands::TOP_MENUS {
                ui.menu_button(top, |ui| self.menu_items(ui, top, &ctx));
            }
        });
    }

    fn menu_item(&mut self, ui: &mut egui::Ui, info: &'static CmdInfo, ctx: &egui::Context) {
        let ok = self.can_run(info.cmd);
        let mut b = egui::Button::new(commands::item_label(info));
        if let Some(k) = info.keys.first() {
            b = b.shortcut_text(*k);
        }
        let r = ui.add_enabled(ok.is_ok(), b);
        let r = match ok {
            Err(why) => r.on_disabled_hover_text(why),
            Ok(()) => r.on_hover_text(info.help),
        };
        if r.clicked() {
            ui.close();
            self.run(info.cmd, ctx);
        }
    }

    /// One menu, built from the command table: groups with separators, submenus for three-level paths, the keys on the right, the
    /// reason in the tooltip of a disabled item.
    pub(super) fn menu_items(&mut self, ui: &mut egui::Ui, top: &str, ctx: &egui::Context) {
        for e in commands::menu_entries(top) {
            match e {
                MenuEntry::Separator => {
                    ui.separator();
                }
                MenuEntry::Item(info) => self.menu_item(ui, info, ctx),
                MenuEntry::Sub(name, items) => {
                    ui.menu_button(name, |ui| {
                        for info in items {
                            match info.cmd {
                                Cmd::HeatOff | Cmd::HeatDensity | Cmd::HeatLevel => self.heat_item(ui, info, ctx),
                                Cmd::LayoutSimple | Cmd::LayoutEditing | Cmd::LayoutReview => {
                                    self.layout_item(ui, commands::item_label(info), ctx);
                                    if info.cmd == Cmd::LayoutReview {
                                        for n in self.settings.layouts.iter().map(|l| l.name.clone()).collect::<Vec<_>>() {
                                            self.layout_item(ui, &n, ctx);
                                        }
                                        ui.separator();
                                    }
                                }
                                _ => self.menu_item(ui, info, ctx),
                            }
                        }
                    });
                }
                MenuEntry::Special("Recent") => self.recent_menu(ui, ctx),
                MenuEntry::Special("Bookmarks") => self.bookmarks_menu(ui),
                MenuEntry::Special("Layers") => {
                    ui.menu_button("Layers", |ui| self.layers_ui(ui));
                }
                MenuEntry::Special(_) => {}
            }
        }
    }

    /// A View > Heatmap entry, ticked when it is the mode in use.
    fn heat_item(&mut self, ui: &mut egui::Ui, info: &'static CmdInfo, ctx: &egui::Context) {
        let mode = match info.cmd {
            Cmd::HeatDensity => crate::heat::Mode::Density,
            Cmd::HeatLevel => crate::heat::Mode::Level,
            _ => crate::heat::Mode::Off,
        };
        let ok = self.can_run(info.cmd);
        let b = egui::Button::new(commands::item_label(info)).selected(self.heat_mode() == mode);
        let r = ui.add_enabled(ok.is_ok(), b);
        let r = match ok {
            Err(why) => r.on_disabled_hover_text(why),
            Ok(()) => r.on_hover_text(info.help),
        };
        if r.clicked() {
            ui.close();
            self.run(info.cmd, ctx);
        }
    }

    /// A layout in the View > Layout menu: ticked when in use, marked "(changed)" when the panels moved since.
    fn layout_item(&mut self, ui: &mut egui::Ui, name: &str, _ctx: &egui::Context) {
        let cur = self.settings.layout_name == name;
        let text = if cur && self.layout_changed() { format!("{name} (changed)") } else { name.to_string() };
        let r = ui.add(egui::Button::new(text).selected(cur));
        if r.on_hover_text("Switch to this layout.").clicked() {
            ui.close();
            self.apply_layout(name);
        }
    }

    // ------------------------------------------------------------------------------------------------------------ layers popover

    /// The layers, grouped: Background, Records (eye and lock per section), Overlays, 3D. The toolbar button and View > Layers show it.
    pub(super) fn layers_ui(&mut self, ui: &mut egui::Ui) {
        fn tick(ui: &mut egui::Ui, flag: &mut bool, key: &str) {
            let help = help_text::LAYERS.iter().find(|h| h.key == key).expect("every layer has a help row");
            ui.checkbox(flag, help.label).on_hover_text(help.tip);
        }
        let heading = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(t).strong().small());
        };
        heading(ui, "Background");
        let l = &mut self.settings.layers;
        tick(ui, &mut l.terrain, "terrain");
        tick(ui, &mut l.minimap, "minimap");
        if l.minimap {
            ui.add(egui::Slider::new(&mut self.settings.minimap_alpha, 0.1..=1.0).text("opacity")).on_hover_text("How opaque the client minimap is.");
        }
        tick(ui, &mut self.settings.layers.dungeon, "dungeon");
        tick(ui, &mut self.settings.layers.objects, "objects");
        ui.separator();
        heading(ui, "Records");
        for (sec, key) in [(SectionId::MobAreas, "areas"), (SectionId::Npcs, "npcs"), (SectionId::Portals, "portals"), (SectionId::Binds, "binds"), (SectionId::Regions, "regions"), (SectionId::SectionB, "b"), (SectionId::SectionA, "a")] {
            let help = help_text::LAYERS.iter().find(|h| h.key == key).expect("every record layer has a help row");
            let (vis, lock) = layer_flags(&mut self.settings, sec);
            ui.horizontal(|ui| {
                if icons::icon_button_sized(ui, if *vis { Icon::Eye } else { Icon::EyeOff }, false, help_text::tip("outliner-eye"), 22.0, true).clicked() {
                    *vis = !*vis;
                }
                if icons::icon_button_sized(ui, if *lock { Icon::Lock } else { Icon::Unlock }, *lock, help_text::tip("outliner-lock"), 22.0, true).clicked() {
                    *lock = !*lock;
                }
                ui.label(help.label).on_hover_text(help.tip);
            });
        }
        ui.separator();
        heading(ui, "Overlays");
        let l = &mut self.settings.layers;
        tick(ui, &mut l.blocked, "blocked");
        tick(ui, &mut l.grid, "grid");
        tick(ui, &mut l.ghosts, "ghosts");
        tick(ui, &mut l.names, "names");
        tick(ui, &mut l.compare, "compare");
        self.heat_controls(ui);
        ui.checkbox(&mut self.settings.overview, "overview inset").on_hover_text(help_text::tip("overview"));
        ui.separator();
        heading(ui, "3D");
        let l = &mut self.settings.layers;
        tick(ui, &mut l.buildings3d, "buildings");
        tick(ui, &mut l.trees3d, "trees");
        ui.add(egui::Slider::new(&mut self.settings.prop_distance, 100.0..=3000.0).logarithmic(true).text("3D object distance")).on_hover_text("Buildings, shapes and trees farther away are not drawn.");
        ui.checkbox(&mut self.v3.cull, "cull backfaces").on_hover_text(help_text::tip("cull"));
        ui.checkbox(&mut self.settings.show_3d_stats, "3D stats").on_hover_text(help_text::tip("show-3d-stats"));
    }

    // ------------------------------------------------------------------------------------------------------------ panels

    /// Every panel of the window except the toolbar, tool palette and status bar.
    pub(super) fn draw_panels(&mut self, ui: &mut egui::Ui) {
        let gen = self.layout_gen;
        let l = self.settings.layout.clone();
        self.r1.rails.clear();
        let pal = theme::pal_ui(ui);
        let rail_frame = Frame::new().fill(pal.rail_bg).inner_margin(Margin::same(2));
        // a size saved on a bigger screen (or typed into the file) starts at most at a share of this window
        let room = ui.available_rect_before_wrap().size();
        let cap_w = |s: f32| s.min((room.x * 0.4).max(180.0));
        let cap_h = |s: f32| s.min((room.y * 0.6).max(70.0));

        // ---- the Dock at the bottom: the full width under the side panels
        if l.dock.visible && l.dock.side == Side::Bottom {
            if l.dock.collapsed {
                let r = egui::Panel::bottom(egui::Id::unique(("rail-dock", gen))).exact_size(RAIL - 2.0).resizable(false).frame(rail_frame).show(ui, |ui| self.dock_rail(ui));
                self.r1.rails.push((Side::Bottom, r.response.rect));
            } else {
                let r = egui::Panel::bottom(egui::Id::unique(("dock-bottom", gen))).resizable(true).default_size(cap_h(l.dock.size)).min_size(70.0).show(ui, |ui| self.dock_body(ui));
                self.remember_size(PanelId::Dock, r.response.rect.height());
            }
        }
        // ---- the sides, outer rails first
        for side in [Side::Left, Side::Right] {
            let rails = self.settings.layout.rails(side);
            if !rails.is_empty() {
                let panel = if side == Side::Left { egui::Panel::left(egui::Id::unique(("rail", 0, gen))) } else { egui::Panel::right(egui::Id::unique(("rail", 1, gen))) };
                let r = panel.exact_size(RAIL).resizable(false).frame(rail_frame).show(ui, |ui| self.side_rail(ui, side, &rails));
                self.r1.rails.push((side, r.response.rect));
            }
            if side == Side::Right && l.dock.visible && !l.dock.collapsed && l.dock.side == Side::Right {
                let r = egui::Panel::right(egui::Id::unique(("dock-right", gen))).resizable(true).default_size(cap_w(l.dock.size)).min_size(220.0).show(ui, |ui| self.dock_body(ui));
                self.remember_size(PanelId::Dock, r.response.rect.width());
            }
            let items = self.settings.layout.stacked(side);
            if !items.is_empty() {
                let width = cap_w(self.settings.layout.panel(items[0]).size);
                let id = egui::Id::unique(("stack", if side == Side::Left { 0 } else { 1 }, gen));
                let panel = if side == Side::Left { egui::Panel::left(id) } else { egui::Panel::right(id) };
                let r = panel.resizable(true).default_size(width).min_size(180.0).show(ui, |ui| self.stack_body(ui, &items));
                let w = r.response.rect.width();
                for p in &items {
                    self.remember_size(*p, w);
                }
            }
        }
    }

    /// Keeps a panel's size when the user dragged its border (a change under a pixel is noise).
    fn remember_size(&mut self, p: PanelId, size: f32) {
        let s = self.settings.layout.panel_mut(p);
        if size.is_finite() && (s.size - size).abs() > 0.75 {
            s.size = size;
        }
    }

    /// Browser and Inspector on one side: one above the other with a drag handle between them.
    fn stack_body(&mut self, ui: &mut egui::Ui, items: &[PanelId]) {
        if let [only] = items {
            self.panel_body(ui, *only);
            return;
        }
        let rect = ui.available_rect_before_wrap();
        let handle_h = 6.0;
        let ratio = self.settings.layout.stack_ratio.clamp(0.15, 0.85);
        let top_h = ((rect.height() - handle_h) * ratio).max(60.0);
        let top = Rect::from_min_size(rect.min, Vec2::new(rect.width(), top_h));
        let handle = Rect::from_min_size(top.left_bottom(), Vec2::new(rect.width(), handle_h));
        let bottom = Rect::from_min_max(handle.left_bottom(), rect.max);
        let resp = ui.interact(handle, egui::Id::unique("stack-handle"), Sense::drag());
        if resp.dragged() {
            let h = (rect.height() - handle_h).max(1.0);
            self.settings.layout.stack_ratio = (ratio + resp.drag_delta().y / h).clamp(0.15, 0.85);
        }
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let pal = theme::pal_ui(ui);
        ui.painter().hline(handle.x_range(), handle.center().y, egui::Stroke::new(1.0, if resp.hovered() { pal.link } else { pal.weak.gamma_multiply(0.5) }));
        for (r, p) in [(top, items[0]), (bottom, items[1])] {
            ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                ui.set_clip_rect(r.intersect(ui.clip_rect()));
                self.panel_body(ui, p);
            });
        }
    }

    fn panel_body(&mut self, ui: &mut egui::Ui, p: PanelId) {
        match p {
            PanelId::Browser => self.browser_body(ui),
            PanelId::Inspector => self.inspector_body(ui),
            PanelId::Dock => self.dock_body(ui),
        }
    }

    /// The three small buttons at the right of a panel header (collapse, other side, close) and the header's right-click menu.
    fn header_buttons(&mut self, ui: &mut egui::Ui, p: PanelId) -> Option<Hdr> {
        let mut act = None;
        let side_tip = if p == PanelId::Dock { "Move to the other side (bottom or right)." } else { "Move to the other side." };
        if icons::icon_button_sized(ui, Icon::Close, false, "Hide this panel.", 20.0, true).clicked() {
            act = Some(Hdr::Close);
        }
        if icons::icon_button_sized(ui, Icon::MoveSide, false, side_tip, 20.0, true).clicked() {
            act = Some(Hdr::Move);
        }
        if icons::icon_button_sized(ui, Icon::Collapse, false, "Collapse to a rail.", 20.0, true).clicked() {
            act = Some(Hdr::Collapse);
        }
        act
    }

    fn do_header(&mut self, p: PanelId, act: Option<Hdr>) {
        match act {
            Some(Hdr::Collapse) => self.settings.layout.set_collapsed(p, true),
            Some(Hdr::Move) => self.settings.layout.move_side(p),
            Some(Hdr::Close) => self.settings.layout.hide(p),
            Some(Hdr::ResetSize) => {
                let d = LayoutState::simple();
                self.settings.layout.panel_mut(p).size = d.panel(p).size;
                self.layout_gen += 1;
            }
            None => {}
        }
    }

    /// A header row: `tabs` draws the left part, the buttons sit at the right. A right click on the row gives the same actions.
    fn header_row(&mut self, ui: &mut egui::Ui, p: PanelId, tabs: impl FnOnce(&mut Self, &mut egui::Ui), extra: impl FnOnce(&mut Self, &mut egui::Ui)) {
        let row = Rect::from_min_size(ui.available_rect_before_wrap().min, Vec2::new(ui.available_width(), 24.0));
        let bg = ui.interact(row, ui.scope_id().with(("header", p.label())), Sense::click());
        let mut act: Option<Hdr> = None;
        bg.context_menu(|ui| {
            if ui.button("Collapse to a rail").clicked() {
                act = Some(Hdr::Collapse);
                ui.close();
            }
            if ui.button("Move to the other side").clicked() {
                act = Some(Hdr::Move);
                ui.close();
            }
            if ui.button("Hide").clicked() {
                act = Some(Hdr::Close);
                ui.close();
            }
            if ui.button("Reset size").clicked() {
                act = Some(Hdr::ResetSize);
                ui.close();
            }
        });
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            tabs(self, ui);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(a) = self.header_buttons(ui, p) {
                    act = Some(a);
                }
                extra(self, ui);
            });
        });
        self.do_header(p, act);
    }

    // ------------------------------------------------------------------------------------------------------------ rails

    fn side_rail(&mut self, ui: &mut egui::Ui, _side: Side, panels: &[PanelId]) {
        ui.spacing_mut().item_spacing = Vec2::new(0.0, 4.0);
        for p in panels {
            match p {
                PanelId::Browser => {
                    let cur = self.settings.layout.browser_tab;
                    for (tab, icon, tip) in [(BrowserTab::Maps, Icon::Maps, "Maps (open the Browser)"), (BrowserTab::Outliner, Icon::Outliner, "Outliner (open the Browser)")] {
                        if icons::icon_button_sized(ui, icon, cur == tab, tip, 24.0, true).clicked() {
                            self.settings.layout.browser_tab = tab;
                            self.settings.layout.show(PanelId::Browser);
                        }
                    }
                }
                PanelId::Inspector => {
                    if icons::icon_button_sized(ui, Icon::Inspector, false, "Inspector (open it)", 24.0, true).clicked() {
                        self.settings.layout.show(PanelId::Inspector);
                    }
                }
                PanelId::Dock => {
                    for (tab, icon, tip) in [(DockTab::Problems, Icon::Problems, "Problems"), (DockTab::Changes, Icon::Changes, "Changes"), (DockTab::Find, Icon::Find, "Find"), (DockTab::Spawns, Icon::Spawns, "Spawns")] {
                        if icons::icon_button_sized(ui, icon, false, tip, 24.0, true).clicked() {
                            self.open_dock(tab);
                        }
                    }
                }
            }
            ui.add_space(6.0);
        }
    }

    /// The Dock collapsed at the bottom: its tab names with counts.
    fn dock_rail(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            for tab in DockTab::ALL {
                let text = self.dock_tab_text(tab);
                if ui.add(egui::Button::new(text).frame(false)).on_hover_text("Open the Dock on this tab.").clicked() {
                    self.open_dock(tab);
                }
            }
        });
    }

    fn dock_tab_text(&self, tab: DockTab) -> String {
        match tab {
            DockTab::Problems => {
                let (e, w, _) = self.docs.get(self.active).map_or((0, 0, 0), |d| shaiya_svmap::tally(&d.findings));
                format!("Problems {e} · {w}")
            }
            DockTab::Changes => {
                let n = self.docs.get(self.active).map_or(0, |d| d.undo.len());
                format!("Changes ({n})")
            }
            DockTab::Find => {
                let nf = self.q3.find.results.len();
                if nf > 0 { format!("Find ({nf})") } else { "Find".to_string() }
            }
            DockTab::Spawns => match self.spawn_tab_count() {
                Some(n) if n > 0 => format!("Spawns ({n})"),
                _ => "Spawns".to_string(),
            },
        }
    }

    // ------------------------------------------------------------------------------------------------------------ Browser

    fn browser_body(&mut self, ui: &mut egui::Ui) {
        let split = self.settings.layout.browser_split;
        self.header_row(
            ui,
            PanelId::Browser,
            |app, ui| {
                if split {
                    ui.label(RichText::new("Browser").strong());
                } else {
                    let mut tab = app.settings.layout.browser_tab;
                    ui.selectable_value(&mut tab, BrowserTab::Maps, "Maps").on_hover_text("The maps of the open folder.");
                    ui.selectable_value(&mut tab, BrowserTab::Outliner, "Outliner").on_hover_text("The records of the open map.");
                    app.settings.layout.browser_tab = tab;
                }
            },
            |app, ui| {
                let r = icons::icon_button_sized(ui, Icon::Chevron, false, "Browser options.", 20.0, true);
                egui::Popup::menu(&r).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                    ui.checkbox(&mut app.settings.layout.browser_split, "Show both").on_hover_text("Maps above the Outliner.");
                    ui.checkbox(&mut app.settings.show_map_sizes, "Show size column").on_hover_text("Each map's side in cells.");
                });
            },
        );
        ui.separator();
        if split {
            self.maps_list(ui, Some(layout::SPLIT_RATIO));
            ui.separator();
            self.outliner_section(ui);
        } else {
            match self.settings.layout.browser_tab {
                BrowserTab::Maps => self.maps_list(ui, None),
                BrowserTab::Outliner => self.outliner_section(ui),
            }
        }
    }

    /// The Maps tab: a filter, then `  1  Erina (Alliance of Light)` rows with a D tag for dungeons and a dot for unsaved changes.
    fn maps_list(&mut self, ui: &mut egui::Ui, share: Option<f32>) {
        if self.folder.is_none() && self.docs.is_empty() {
            ui.label("No folder open.");
            if ui.button("Open map folder...").clicked() {
                let ctx = ui.ctx().clone();
                self.run(Cmd::OpenFolder, &ctx);
            }
            return;
        }
        ui.add(egui::TextEdit::singleline(&mut self.map_filter).hint_text("filter by id or name").desired_width(f32::INFINITY)).on_hover_text(help_text::tip("map-filter"));
        let h = ui.available_height();
        let rows = self.map_rows();
        let mut open = None;
        let row_h = 18.0;
        let max_h = match share {
            Some(s) => (ui.available_height() / (1.0 - s) * s).max(80.0),
            None => h.max(80.0),
        };
        let sizes = self.settings.show_map_sizes;
        egui::ScrollArea::vertical().id_salt("maplist").max_height(max_h).auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            for r in range {
                let (id, name, size, kind) = &rows[r];
                let is_open = self.docs.iter().any(|d| d.map_id == Some(*id));
                let dirty = self.docs.iter().any(|d| d.map_id == Some(*id) && d.is_dirty());
                let badge = self.safety.folder_check.result.as_ref().and_then(|r| r.tally_of(*id)).map(crate::problems::badge).filter(|b| !b.is_empty()).map_or(String::new(), |b| format!("  [{b}]"));
                let tag = if *kind == Some(true) { "D" } else { " " };
                let size_txt = if sizes { format!(" {:>4}", size.map_or("-".to_string(), |s| s.to_string())) } else { String::new() };
                let text = format!("{}{id:>3}{size_txt} {tag} {name}{badge}", if dirty { "•" } else { " " });
                let sel = self.selected_map_row == Some(*id) || (is_open && self.docs.get(self.active).and_then(|d| d.map_id) == Some(*id));
                let resp = ui.selectable_label(sel, RichText::new(text).monospace());
                let kind_txt = match kind {
                    Some(true) => "dungeon",
                    Some(false) => "field",
                    None => "kind unknown",
                };
                let path = self.folder.as_ref().and_then(|f| f.maps.get(id)).map(|m| m.path.display().to_string()).unwrap_or_default();
                let resp = resp.on_hover_text(format!("Map {id} · {kind_txt} · {} cells\n{path}", size.map_or("?".to_string(), |s| s.to_string())));
                if resp.clicked() {
                    self.selected_map_row = Some(*id);
                }
                if resp.double_clicked() {
                    open = Some(*id);
                }
            }
        });
        if let Some(id) = open {
            self.open_map_id(id);
        }
    }

    /// The Outliner tab: its filter and the sections.
    fn outliner_section(&mut self, ui: &mut egui::Ui) {
        if self.docs.is_empty() {
            ui.label("No map open.");
            return;
        }
        let filter_resp = ui.add(egui::TextEdit::singleline(&mut self.outliner_filter).hint_text("filter by id, name or monster").desired_width(f32::INFINITY)).on_hover_text(help_text::tip("outliner-filter"));
        if std::mem::take(&mut self.focus_outliner) {
            filter_resp.request_focus();
        }
        self.outliner(ui);
    }

    // ------------------------------------------------------------------------------------------------------------ Inspector

    fn inspector_body(&mut self, ui: &mut egui::Ui) {
        self.header_row(ui, PanelId::Inspector, |_, ui| {
            ui.label(RichText::new("Inspector").strong());
        }, |_, _| {});
        ui.separator();
        self.right_panel(ui);
    }

    // ------------------------------------------------------------------------------------------------------------ Dock

    fn dock_body(&mut self, ui: &mut egui::Ui) {
        self.header_row(
            ui,
            PanelId::Dock,
            |app, ui| {
                for tab in DockTab::ALL {
                    let text = app.dock_tab_text(tab);
                    let cur = app.settings.layout.dock_tab == tab;
                    let r = ui.selectable_label(cur, text);
                    let r = match tab {
                        DockTab::Find => r.on_hover_text("Search every map (Ctrl+Shift+F)."),
                        DockTab::Changes => r.on_hover_text("History and compare."),
                        DockTab::Problems => r.on_hover_text("What the checks found."),
                        DockTab::Spawns => r.on_hover_text("Spawn table, level range and spawn findings."),
                    };
                    if r.clicked() {
                        app.settings.layout.dock_tab = tab;
                    }
                }
            },
            |_, _| {},
        );
        if self.settings.layout.dock_tab == DockTab::Problems {
            ui.horizontal(|ui| self.problems_controls(ui));
        }
        ui.separator();
        match self.settings.layout.dock_tab {
            DockTab::Problems => self.problems_body(ui),
            DockTab::Changes => self.changes_body(ui),
            DockTab::Find => self.find_body(ui),
            DockTab::Spawns => self.spawns_body(ui),
        }
    }

    // ------------------------------------------------------------------------------------------------------------ tool palette

    /// Why a tool cannot be used now (None = usable).
    pub(super) fn tool_blocked(&self, t: Tool) -> Option<&'static str> {
        let ro = self.docs.get(self.active).is_some_and(|d| d.read_only);
        if ro && !matches!(t, Tool::Select | Tool::Measure) {
            return Some("read-only tab");
        }
        if t == Tool::Terrain {
            return self.terrain_blocked();
        }
        if self.mode3d && matches!(t, Tool::Paint | Tool::Scatter) {
            return Some("2D only");
        }
        None
    }

    /// The vertical icon strip at the left edge of the viewport.
    pub(super) fn tool_palette(&mut self, ui: &mut egui::Ui) {
        let pal = theme::pal_ui(ui);
        let mut n = 0;
        egui::Panel::left("tools").exact_size(PALETTE_W).resizable(false).frame(Frame::new().fill(pal.rail_bg).inner_margin(Margin::same(4))).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 4.0);
            for t in Tool::ALL {
                let why = self.tool_blocked(t);
                let tip = match why {
                    Some(w) => format!("{} ({}): {w}", t.label(), tool_key(t)),
                    None => format!("{} ({})\n{}", t.label(), tool_key(t), help_text::tool_tip(t)),
                };
                let r = icons::icon_button_sized(ui, tool_icon(t), self.tool == t, &tip, 28.0, why.is_none());
                if r.clicked() && why.is_none() {
                    self.tool = t;
                }
                n += 1;
            }
        });
        self.r1.drawn_palette = Some(n);
    }

    // ------------------------------------------------------------------------------------------------------------ toolbar

    /// How wide the options of a tool are (the toolbar moves them into a popover when they do not fit).
    fn options_width(&self) -> f32 {
        match self.tool {
            Tool::Box => 440.0,
            Tool::Paint => 660.0,
            Tool::Npc => 230.0,
            Tool::Scatter => 560.0,
            Tool::Place => 400.0,
            Tool::Terrain => 780.0,
            Tool::Measure if !self.mode3d => 90.0,
            _ => 0.0,
        }
    }

    fn tool_options(&mut self, ui: &mut egui::Ui) {
        match self.tool {
            Tool::Box => {
                ui.label("draw").on_hover_text(help_text::tip("draw-target"));
                egui::ComboBox::from_id_salt("draw-target").selected_text(self.draw_target.label()).show_ui(ui, |ui| {
                    for t in DrawTarget::ALL {
                        ui.selectable_value(&mut self.draw_target, t, t.label());
                    }
                });
                if self.draw_target == DrawTarget::MobArea {
                    self.template_toolbar(ui);
                }
            }
            Tool::Paint => self.paint_toolbar(ui),
            Tool::Scatter => self.scatter_toolbar(ui),
            Tool::Place => self.place_toolbar(ui),
            Tool::Terrain => self.terrain_toolbar(ui),
            Tool::Measure if !self.mode3d => {
                ui.checkbox(&mut self.vp.radius_mode, "Radius").on_hover_text(help_text::tip("measure-radius"));
            }
            Tool::Npc => {
                ui.label("new NPC type").on_hover_text(help_text::tip("npc-type"));
                ui.add(egui::DragValue::new(&mut self.npc_template.0).range(0..=13));
                ui.label("id");
                ui.add(egui::DragValue::new(&mut self.npc_template.1).range(0..=65535));
            }
            _ => {}
        }
    }

    /// One row: 2D | 3D, Snap, Layers, level chip, the active tool's options, then the problems and client chips at the right.
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let show_client = match (&self.client_path, &self.client) {
            (None, _) => true,
            (Some(_), Some(c)) => !c.problems.is_empty() || !self.client_notes.is_empty(),
            (Some(_), None) => false,
        };
        let reserve = 120.0 + if show_client { 130.0 } else { 0.0 };
        let mut overflow = false;
        self.r1.chip_rect = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // 2D | 3D
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if ui.add(egui::Button::selectable(!self.mode3d, "2D")).on_hover_text(help_text::tip("view-2d")).clicked() {
                    self.set_mode3d(false);
                }
                if ui.add(egui::Button::selectable(self.mode3d, "3D")).on_hover_text(help_text::tip("view-3d")).clicked() {
                    self.set_mode3d(true);
                }
            });
            // Snap
            let popover = MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside);
            let (r, _) = MenuButton::new(format!("Snap {}", self.settings.snap)).config(popover.clone()).ui(ui, |ui| {
                ui.label(RichText::new("Step").strong().small());
                ui.horizontal_wrapped(|ui| {
                    for s in self.settings.snap_steps.clone() {
                        ui.selectable_value(&mut self.settings.snap, s, format!("{s}"));
                    }
                });
                ui.checkbox(&mut self.drop_on_move, "ground on move").on_hover_text(help_text::tip("ground-on-move"));
            });
            r.on_hover_text(help_text::tip("snap"));
            // Layers
            let (r, _) = MenuButton::new("Layers").config(popover).ui(ui, |ui| self.layers_ui(ui));
            r.on_hover_text(help_text::tip("layers"));
            // level chip
            self.level_filter_button(ui);
            // the active tool's options; a very narrow window also moves the chips into the "..." popover
            let need = self.options_width();
            let room = ui.available_width() - reserve;
            let narrow = room < 0.0;
            if need > 0.0 && room >= need {
                ui.separator();
                self.tool_options(ui);
            } else if need > 0.0 || narrow {
                overflow = need > 0.0;
                let (r, _) = MenuButton::new("...").config(MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside)).ui(ui, |ui| {
                    if need > 0.0 {
                        ui.label(RichText::new(format!("{} options", self.tool.label())).strong().small());
                        self.tool_options(ui);
                    }
                    if narrow {
                        if need > 0.0 {
                            ui.separator();
                        }
                        self.problems_chip(ui);
                        if show_client {
                            self.client_chip(ui);
                        }
                    }
                });
                r.on_hover_text(if need > 0.0 { "Options of the active tool." } else { "Problems and client." });
            }
            // the chips, right
            if !narrow {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if show_client {
                        self.client_chip(ui);
                    }
                    self.problems_chip(ui);
                });
            }
        });
        self.r1.toolbar_overflow = overflow;
    }

    /// The text of the client chip and its hover: the problems (client files that did not load) and the episode notes.
    pub(super) fn client_chip_text(&self) -> (String, String) {
        let np = self.client.as_ref().map_or(0, |c| c.problems.len());
        let first: Vec<String> = self.client.as_ref().map(|c| c.problems.iter().take(8).cloned().collect()).unwrap_or_default();
        let label = if np > 0 { format!("client: {np} problems") } else { "client: notes".to_string() };
        let mut hover = String::new();
        if np > 0 {
            hover.push_str(&format!("Client files that did not load:\n{}", first.join("\n")));
        }
        if !self.client_notes.is_empty() {
            if !hover.is_empty() {
                hover.push_str("\n\n");
            }
            hover.push_str("This client differs from US EP4.5, the version the editor was verified on:\n");
            hover.push_str(&self.client_notes.join("\n"));
        }
        (label, hover)
    }

    fn client_chip(&mut self, ui: &mut egui::Ui) {
        let pal = theme::pal_ui(ui);
        match &self.client_path {
            Some(_) => {
                let (label, hover) = self.client_chip_text();
                ui.label(RichText::new(label).color(pal.warning)).on_hover_text(hover);
            }
            None => {
                let r = ui.add(egui::Label::new(RichText::new("client: none").color(pal.link).underline()).sense(Sense::click()));
                if r.on_hover_text(help_text::tip("client-none")).clicked() {
                    self.open_setup();
                    if let Some(w) = self.wizard.as_mut() {
                        w.step = 1;
                    }
                }
            }
        }
    }

    /// The error dot and warning triangle with their counts (a count of zero is left out; no findings at all says so).
    fn problems_chip(&mut self, ui: &mut egui::Ui) {
        let Some(d) = self.docs.get(self.active) else { return };
        let (e, w, i) = shaiya_svmap::tally(&d.findings);
        let pal = theme::pal_ui(ui);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let mut parts: Vec<(Option<Icon>, Color32, String)> = Vec::new();
        if e > 0 {
            parts.push((Some(Icon::ErrorDot), pal.error, e.to_string()));
        }
        if w > 0 {
            parts.push((Some(Icon::WarnTriangle), pal.warning, w.to_string()));
        }
        if parts.is_empty() {
            parts.push((Some(Icon::ErrorDot), pal.ok, "no problems".to_string()));
        }
        let galleys: Vec<_> = parts.iter().map(|(_, c, t)| ui.painter().layout_no_wrap(t.clone(), font.clone(), if e + w == 0 { pal.weak } else { *c })).collect();
        let width: f32 = galleys.iter().map(|g| 16.0 + 4.0 + g.size().x + 8.0).sum::<f32>();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width.max(24.0), 22.0), Sense::click());
        self.r1.chip_rect = Some(rect);
        if resp.hovered() {
            ui.painter().rect_filled(rect, 4.0, pal.hover);
        }
        let mut x = rect.left() + 2.0;
        for ((icon, color, _), g) in parts.iter().zip(galleys) {
            if let Some(ic) = icon {
                icons::paint(ui.painter(), Rect::from_min_size(egui::pos2(x, rect.center().y - 8.0), Vec2::splat(16.0)), *ic, *color);
            }
            x += 20.0;
            let size = g.size();
            ui.painter().galley(egui::pos2(x, rect.center().y - size.y / 2.0), g, pal.text);
            x += size.x + 8.0;
        }
        let tip = format!("{e} error{}, {w} warning{}, {i} info. Click to show or hide Problems.", if e == 1 { "" } else { "s" }, if w == 1 { "" } else { "s" });
        if resp.on_hover_text(tip).clicked() {
            if self.dock_showing(DockTab::Problems) {
                self.settings.layout.set_collapsed(PanelId::Dock, true);
            } else {
                self.open_dock(DockTab::Problems);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------------------ status bar

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let mut copy: Option<String> = None;
        let pal = theme::pal_ui(ui);
        ui.horizontal(|ui| {
            let assets = self.current_assets();
            let uid = self.docs.get(self.active).map(|d| d.uid);
            let live = self.vp.hover.is_some();
            if let (Some(u), Some(h)) = (uid, self.vp.hover) {
                self.last_hover = Some((u, h));
            }
            let shown = self.last_hover.filter(|(u, _)| Some(*u) == uid).map(|(_, p)| p);
            if let (Some(d), Some(p)) = (self.docs.get(self.active), shown) {
                let weak = |t: String| if live { RichText::new(t) } else { RichText::new(t).weak() };
                let ground = assets.as_ref().and_then(|a| a.ground.height(p[0], p[1])).map_or("none".to_string(), |h| format!("{h:.1}"));
                let r = ui.add(egui::Label::new(weak(format!("x {:.1}  z {:.1}", p[0], p[1]))).sense(Sense::click()));
                if r.on_hover_text(format!("{} Ground height {ground}.", help_text::tip("status-cursor"))).clicked() {
                    copy = Some(position_text(p));
                }
                let cell = shaiya_svmap::validate::cell_of(d.map.size, p[0], p[1]);
                let cell_txt = match cell {
                    Some((x, z)) => format!("cell {} {}", z as u64 * d.map.size as u64 + x as u64, if d.map.blocked.get(x, z) { "blocked" } else { "free" }),
                    None => "outside".to_string(),
                };
                ui.label(weak(cell_txt)).on_hover_text(help_text::tip("status-cell"));
                if let Some(i) = crate::view2d::region_at(&d.map, p[0], p[1]) {
                    let look = Lookups { rules: self.client.as_deref(), assets: assets.as_deref(), map_id: d.map_id, names: d.part().map(|p| p.names()) };
                    ui.label(weak(format!("region \"{}\"", look.region_label(i, &d.map.regions[i])))).on_hover_text(help_text::tip("status-region"));
                }
                if let Some(t) = self.heat_status(p) {
                    ui.label(weak(t)).on_hover_text(help_text::tip("status-heat"));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.docs.get(self.active).and_then(|d| d.verified_at).is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
                    ui.label(RichText::new("Saved and verified").color(pal.ok)).on_hover_text(help_text::tip("status-verified"));
                    ui.ctx().request_repaint_after(Duration::from_secs(1));
                }
                self.client_chips(ui, &pal);
                if self.no_data_folder {
                    ui.label(RichText::new("No data folder: settings and backups are off; use --data-dir").color(pal.warning)).on_hover_text("The editor could not find a place for its settings, log, recovery files and backups. Start it with --data-dir <folder> to name one.");
                }
                if self.settings.show_frame_time {
                    ui.label(RichText::new(format!("{:.1} ms/frame (ui {:.2} ms)", self.frame_ms, self.ui_ms)).weak()).on_hover_text(help_text::tip("status-frame"));
                }
                if let Some(e) = &self.safety.autosave_error {
                    ui.label(RichText::new(format!("autosave failed: {e}")).color(pal.warning)).on_hover_text("Unsaved work could not be written to the recovery folder. The editor tries again at the next autosave.");
                }
                if let Some(Slot::Loading(_)) = self.docs.get(self.active).and_then(|d| d.map_id).and_then(|id| self.slots.get(&id)) {
                    ui.label("loading client data...");
                }
                if self.settings.hints && live {
                    let hint = if self.mode3d { help_text::HINT_3D } else { help_text::tool_hint(self.tool) };
                    ui.add(egui::Label::new(RichText::new(hint).weak()).truncate()).on_hover_text(help_text::tip("hint-line"));
                }
            });
        });
        self.r1.drawn_status = true;
        if let Some(t) = copy {
            ui.ctx().copy_text(t.clone());
            self.toast(format!("copied {t}"));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------- R4: measure

impl App {
    /// The measurement of the active tab in the view that is showing (2D or 3D).
    fn measure_ref(&self) -> Option<&crate::measure::Measure> {
        let d = self.docs.get(self.active)?;
        let m = if self.mode3d { &self.v3.states.get(&d.uid)?.measure } else { &self.vp.measure };
        m.active().then_some(m)
    }

    /// What Ctrl+C copies with the Measure tool: "length 123.4 (3 segments), area 812.0". None when no measurement is there.
    pub(super) fn measure_text(&self) -> Option<String> {
        if self.tool != Tool::Measure {
            return None;
        }
        self.measure_ref()?.copy_text()
    }

    /// Esc: clears the measurement of the showing view. False when there was none.
    pub(super) fn clear_measure(&mut self) -> bool {
        if self.measure_ref().is_none() {
            return false;
        }
        let Some(d) = self.docs.get(self.active) else { return false };
        if self.mode3d {
            if let Some(s) = self.v3.states.get_mut(&d.uid) {
                s.measure.clear();
            }
        } else {
            self.vp.measure.clear();
        }
        true
    }
}
