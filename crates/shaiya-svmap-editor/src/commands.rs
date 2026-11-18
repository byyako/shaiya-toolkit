//! The command table: one list of everything the editor can do from a menu, a key or the command palette. The menus, the key
//! handling, the palette and the Help window's shortcut page all read it, so they cannot drift apart.
//!
//! `menu` is the full menu path as the menu shows it (`File > Open file...`); the Help guide writes a command as
//! `**File > Open file...**` and the Help window turns such a bold path into a link that runs the command. An empty `menu` means
//! the command is only on a key and in the palette. `group` splits a menu into blocks (a separator between two groups).
//! `keys` are the default keys (the first is shown in the menu); the typed form is `Ctrl+Shift+P`.

use crate::view2d::Tool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    OpenFile,
    OpenFolder,
    OpenClient,
    ReloadClient,
    QuickOpen,
    NewFromClient,
    Save,
    SaveAs,
    SaveAll,
    RestoreBackup,
    CloseTab,
    NextTab,
    PrevTab,
    Exit,
    Undo,
    Redo,
    Copy,
    Paste,
    PasteInPlace,
    Duplicate,
    Delete,
    DropToGround,
    SelectAll,
    SelectSimilar,
    SelectMatching,
    FindInMaps,
    AddBookmark,
    PortalLinks,
    ExportAreasCsv,
    ExportNpcsCsv,
    ExportPortalsCsv,
    ExportBindsCsv,
    ExportRegionsCsv,
    ExportAllCsv,
    ExportFolderAreasCsv,
    ExportJson,
    ExportPng,
    FocusOutliner,
    Preferences,
    FrameSelection,
    FrameMap,
    Toggle3d,
    GoTo,
    Palette,
    CopyPosition,
    CopyGmMove,
    Tool(Tool),
    ValidateNow,
    GenerateBlocked,
    SyncFromClient,
    SyncSelected,
    StageClientMap,
    Help,
    Shortcuts,
    Setup,
    OpenDataFolder,
    ShowBrowser,
    ShowInspector,
    ShowDock,
    ViewportOnly,
    HeatCycle,
    HeatOff,
    HeatDensity,
    HeatLevel,
    Rulers,
    SpawnCheck,
    BulkEdit,
    LayoutSimple,
    LayoutEditing,
    LayoutReview,
    SaveLayout,
    ManageLayouts,
    WorldView,
    CompareFolders,
    MapIniEditor,
    AddMap,
    ShareClient,
}

pub struct CmdInfo {
    pub cmd: Cmd,
    /// A stable name (the palette's tooltip, the log, tests).
    pub id: &'static str,
    /// Name in the palette.
    pub label: &'static str,
    /// Full menu path, or "" for none.
    pub menu: &'static str,
    pub group: u8,
    pub keys: &'static [&'static str],
    pub help: &'static str,
}

const fn c(cmd: Cmd, id: &'static str, label: &'static str, menu: &'static str, group: u8, keys: &'static [&'static str], help: &'static str) -> CmdInfo {
    CmdInfo { cmd, id, label, menu, group, keys, help }
}

pub const TOP_MENUS: [&str; 5] = ["File", "Edit", "View", "Tools", "Help"];

static ALL: [CmdInfo; 85] = [
    // ---- File
    c(Cmd::OpenFile, "open-file", "Open file", "File > Open file...", 0, &["Ctrl+O"], "Open one .svmap file in a new tab."),
    c(Cmd::OpenFolder, "open-folder", "Open map folder", "File > Open folder...", 0, &[], "Open the server map folder. Turns on the cross-map checks."),
    c(Cmd::OpenClient, "open-client", "Open client folder", "File > Open client folder...", 0, &[], "Open the client folder for names, terrain and checks."),
    c(Cmd::QuickOpen, "quick-open", "Quick open map", "File > Quick open map...", 0, &["Ctrl+P"], "Type a map number or name to open it."),
    c(Cmd::Save, "save", "Save", "File > Save", 1, &["Ctrl+S"], "Save the map. The first save also keeps a .bak copy."),
    c(Cmd::SaveAs, "save-as", "Save as", "File > Save as...", 1, &["Ctrl+Shift+S"], "Save the map under another name or in another folder."),
    c(Cmd::SaveAll, "save-all", "Save all", "File > Save all", 1, &["Ctrl+Alt+S"], "Save every map with unsaved changes."),
    c(Cmd::CloseTab, "close-tab", "Close tab", "File > Close tab", 1, &["Ctrl+W"], "Close this map. Asks when it has unsaved changes."),
    c(Cmd::NewFromClient, "new-from-client", "New map from client map", "File > New from client map...", 2, &[], "Start a new unsaved map from a client map."),
    c(Cmd::RestoreBackup, "restore-backup", "Restore from backup", "File > Restore from backup...", 2, &[], "Open or restore a timestamped copy of this map."),
    c(Cmd::ExportAreasCsv, "export-areas-csv", "Export monster areas CSV", "File > Export > Monster areas CSV...", 3, &[], "The map's monster areas as a CSV file."),
    c(Cmd::ExportNpcsCsv, "export-npcs-csv", "Export NPCs CSV", "File > Export > NPCs CSV...", 3, &[], "The map's NPCs as a CSV file."),
    c(Cmd::ExportPortalsCsv, "export-portals-csv", "Export portals CSV", "File > Export > Portals CSV...", 3, &[], "The map's portals as a CSV file."),
    c(Cmd::ExportBindsCsv, "export-binds-csv", "Export bind areas CSV", "File > Export > Bind areas CSV...", 3, &[], "The map's bind areas as a CSV file."),
    c(Cmd::ExportRegionsCsv, "export-regions-csv", "Export regions CSV", "File > Export > Regions CSV...", 3, &[], "The map's regions as a CSV file."),
    c(Cmd::ExportAllCsv, "export-all-csv", "Export all sections CSV", "File > Export > All sections CSV...", 3, &[], "All five sections, one CSV file each, into a folder."),
    c(Cmd::ExportFolderAreasCsv, "export-folder-areas-csv", "Export every map's monster areas CSV", "File > Export > Folder monster areas CSV...", 3, &[], "The monster areas of every map in one CSV file."),
    c(Cmd::ExportJson, "export-json", "Export map JSON", "File > Export > Map JSON...", 3, &[], "The map as a JSON document."),
    c(Cmd::ExportPng, "export-png", "Export view as PNG", "File > Export > View as PNG...", 3, &[], "The 2D view as a picture."),
    c(Cmd::ReloadClient, "reload-client", "Reload client data", "File > Reload client data", 4, &[], "Read the client files again. Open tabs stay."),
    c(Cmd::Exit, "exit", "Exit", "File > Exit", 5, &[], "Close the editor. Asks about unsaved maps."),
    // ---- Edit
    c(Cmd::Undo, "undo", "Undo", "Edit > Undo", 0, &["Ctrl+Z"], "Undo the last step of this map."),
    c(Cmd::Redo, "redo", "Redo", "Edit > Redo", 0, &["Ctrl+Y", "Ctrl+Shift+Z"], "Redo the step that was undone."),
    c(Cmd::Copy, "copy", "Copy", "Edit > Copy", 1, &["Ctrl+C"], "Copy the selected records. They paste into other maps too."),
    c(Cmd::Paste, "paste", "Paste", "Edit > Paste", 1, &["Ctrl+V"], "Paste at the cursor. Ctrl+V also reads another editor window."),
    c(Cmd::PasteInPlace, "paste-in-place", "Paste in place", "Edit > Paste in place", 1, &["Ctrl+Shift+V"], "Paste where the records were copied from."),
    c(Cmd::Duplicate, "duplicate", "Duplicate", "Edit > Duplicate", 1, &["Ctrl+D"], "Copy the selection next to itself."),
    c(Cmd::Delete, "delete", "Delete", "Edit > Delete", 1, &["Del"], "Delete the selected records."),
    c(Cmd::DropToGround, "drop-to-ground", "Drop to ground", "Edit > Drop to ground", 1, &["G"], "Set the selection's height to the ground."),
    c(Cmd::SelectAll, "select-all", "Select all", "Edit > Select all", 2, &["Ctrl+A"], "Select every record of the selected sections, or every visible one."),
    c(Cmd::SelectSimilar, "select-similar", "Select similar", "Edit > Select similar", 2, &[], "Select the records with the same section and key."),
    c(Cmd::SelectMatching, "select-matching", "Select matching", "Edit > Select matching...", 2, &[], "Open Find for this map. Select all on this map selects its hits."),
    c(Cmd::FindInMaps, "find-in-maps", "Find in all maps", "Edit > Find in all maps...", 2, &["Ctrl+Shift+F"], "Search every map by monster, NPC, portal, region, rule or level."),
    c(Cmd::FocusOutliner, "focus-outliner", "Filter the outliner", "Edit > Filter outliner", 3, &["Ctrl+F"], "Jump to the outliner's filter box."),
    c(Cmd::Preferences, "preferences", "Preferences", "Edit > Preferences...", 4, &["Ctrl+,"], "Folders, session, hints, scale, theme and snap steps."),
    // ---- View
    c(Cmd::Toggle3d, "toggle-3d", "Switch 2D / 3D", "View > Switch 2D / 3D", 0, &["Tab"], "Switch between the 2D editor and the 3D view."),
    c(Cmd::FrameSelection, "frame-selection", "Frame selection", "View > Frame selection", 0, &["F"], "Zoom to the selected records."),
    c(Cmd::FrameMap, "frame-map", "Frame map", "View > Frame map", 0, &["Home"], "Fit the whole map in the view."),
    c(Cmd::GoTo, "go-to", "Go to", "View > Go to...", 0, &["Ctrl+G"], "Go to a position, cell, record, map or bookmark."),
    c(Cmd::Palette, "palette", "Command palette", "View > Command palette...", 0, &["Ctrl+Shift+P"], "Search every command by name."),
    c(Cmd::WorldView, "world-view", "World view", "View > World view", 5, &["Ctrl+Shift+M"], "Every map of the folder as a card, with its portal links."),
    c(Cmd::ShowBrowser, "show-browser", "Show or hide the Browser", "View > Panels > Browser", 1, &["Ctrl+1"], "Show or hide the Browser (maps and outliner)."),
    c(Cmd::ShowInspector, "show-inspector", "Show or hide the Inspector", "View > Panels > Inspector", 1, &["Ctrl+2"], "Show or hide the Inspector."),
    c(Cmd::ShowDock, "show-dock", "Show or hide the Dock", "View > Panels > Dock", 1, &["Ctrl+3"], "Show or hide the Dock (problems, changes, find, spawns)."),
    c(Cmd::ViewportOnly, "viewport-only", "Viewport only", "View > Panels > Viewport only", 1, &["F11"], "Hide every panel. F11 again brings them back."),
    c(Cmd::LayoutSimple, "layout-simple", "Layout: Simple", "View > Layout > Simple", 1, &[], "Browser and Inspector. The Dock is hidden."),
    c(Cmd::LayoutEditing, "layout-editing", "Layout: Editing", "View > Layout > Editing", 1, &[], "Outliner on the left. The Dock is a rail."),
    c(Cmd::LayoutReview, "layout-review", "Layout: Review", "View > Layout > Review", 1, &[], "Problems open at the bottom."),
    c(Cmd::SaveLayout, "save-layout", "Save layout", "View > Layout > Save layout...", 1, &[], "Keep the panels as they are now under a name."),
    c(Cmd::ManageLayouts, "manage-layouts", "Manage layouts", "View > Layout > Manage layouts...", 1, &[], "Rename or delete saved layouts."),
    c(Cmd::HeatOff, "heat-off", "Heatmap: off", "View > Heatmap > Off", 1, &[], "Hide the heatmap."),
    c(Cmd::HeatDensity, "heat-density", "Heatmap: density", "View > Heatmap > Density", 1, &[], "Monsters per tile, from the monster areas."),
    c(Cmd::HeatLevel, "heat-level", "Heatmap: level", "View > Heatmap > Level", 1, &[], "Mean monster level per tile. Needs the client data."),
    c(Cmd::HeatCycle, "heat-cycle", "Cycle the heatmap", "", 1, &["H"], "Off, density, level, off."),
    c(Cmd::Rulers, "rulers", "Rulers", "View > Rulers", 1, &["Ctrl+R"], "Show or hide the rulers at the top and left of the 2D view."),
    c(Cmd::AddBookmark, "add-bookmark", "Add bookmark", "View > Add bookmark...", 2, &["Ctrl+B"], "Keep this view of the map under a name."),
    c(Cmd::NextTab, "next-tab", "Next tab", "View > Next tab", 3, &["Ctrl+Tab"], "Show the next open map."),
    c(Cmd::PrevTab, "prev-tab", "Previous tab", "View > Previous tab", 3, &["Ctrl+Shift+Tab"], "Show the previous open map."),
    c(Cmd::CopyPosition, "copy-position", "Copy cursor position", "View > Copy cursor position", 4, &["Ctrl+Shift+C"], "Copy x and z of the cursor, selection or view centre."),
    c(Cmd::CopyGmMove, "copy-gm-move", "Copy as GM move command", "View > Copy as GM move command", 4, &[], "Copy /bmove x z map for the game chat."),
    c(Cmd::Tool(Tool::Select), "tool-select", "Tool: Select", "", 0, &["1"], "Click selects, drag moves, drag on empty ground selects several."),
    c(Cmd::Tool(Tool::Box), "tool-box", "Tool: Box", "", 0, &["2"], "Drag out a monster area, bind area or region."),
    c(Cmd::Tool(Tool::Npc), "tool-npc", "Tool: NPC", "", 0, &["3"], "Click to place an NPC."),
    c(Cmd::Tool(Tool::Portal), "tool-portal", "Tool: Portal", "", 0, &["4"], "Click to place a portal. Asks first."),
    c(Cmd::Tool(Tool::Paint), "tool-paint", "Tool: Paint-blocked", "", 0, &["5"], "Paint blocked cells."),
    c(Cmd::Tool(Tool::Measure), "tool-measure", "Tool: Measure", "", 0, &["6"], "Click points or drag to measure a distance."),
    c(Cmd::Tool(Tool::Scatter), "tool-scatter", "Tool: Scatter", "", 0, &["7"], "Fill a region with monster areas."),
    c(Cmd::Tool(Tool::Place), "tool-place", "Tool: Placements", "", 0, &["8"], "Edit buildings, shapes and trees of the client map."),
    c(Cmd::Tool(Tool::Terrain), "tool-terrain", "Tool: Terrain", "", 0, &["9"], "Change the ground height of the client map."),
    // ---- Tools
    c(Cmd::SpawnCheck, "spawn-check", "Spawn check", "Tools > Spawns > Spawn check", 0, &[], "Open the Spawns tab: spawn table, level range and findings."),
    c(Cmd::BulkEdit, "bulk-edit", "Bulk edit spawns", "Tools > Spawns > Bulk edit...", 0, &[], "Change the monster areas of many maps with one rule."),
    c(Cmd::PortalLinks, "portal-links", "Portal links", "Tools > Maps > Portal links...", 1, &[], "Every portal of the folder and where it leads."),
    c(Cmd::CompareFolders, "compare-folders", "Compare folders", "Tools > Maps > Compare folders...", 1, &[], "Compare this map folder with another one, map by map."),
    c(Cmd::MapIniEditor, "map-ini-editor", "Map.ini editor", "Tools > Maps > Map.ini editor...", 1, &[], "Edit the zones of the server's Map.ini and MapWar.ini."),
    c(Cmd::AddMap, "add-map", "Add a map", "Tools > Maps > Add a map...", 1, &[], "Make a new map: client files, server map and a Map.ini zone."),
    c(Cmd::GenerateBlocked, "generate-blocked", "Generate blocked cells", "Tools > Client > Generate blocked cells...", 2, &[], "Work out blocked cells from the client, with a preview."),
    c(Cmd::SyncFromClient, "sync-from-client", "Sync from client", "Tools > Client > Sync from client...", 2, &[], "Compare with the client map and take either side per row."),
    c(Cmd::SyncSelected, "sync-selected", "Sync selected record from client", "Tools > Client > Sync selected record from client", 2, &[], "Take the .wld's values for the selected record."),
    c(Cmd::StageClientMap, "stage-client-map", "Stage client map", "Tools > Client > Stage client map...", 2, &[], "Write a verified copy of this map's .wld to the staging folder."),
    c(Cmd::ShareClient, "share-client", "Share client changes", "Tools > Client > Share client changes...", 2, &[], "Export a patch, install into a test client, copy loose files."),
    c(Cmd::ValidateNow, "validate-now", "Validate now", "Tools > Validate now", 3, &[], "Run every check again on all open maps."),
    // ---- Help
    c(Cmd::Help, "help", "User guide", "Help > User guide", 0, &["F1"], "Open the guide at the page for the current tool."),
    c(Cmd::Shortcuts, "shortcuts", "Keyboard shortcuts", "Help > Keyboard shortcuts", 0, &["?"], "A compact list of every key."),
    c(Cmd::Setup, "setup", "Run setup", "Help > Setup...", 1, &[], "Pick the map, client and reference folders again."),
    c(Cmd::OpenDataFolder, "open-data-folder", "Open data folder", "Help > Open data folder", 1, &[], "Open the settings and log folder in Explorer."),
];

/// Every command, in menu order.
pub fn all() -> &'static [CmdInfo] {
    &ALL
}

pub fn info(cmd: Cmd) -> &'static CmdInfo {
    ALL.iter().find(|i| i.cmd == cmd).expect("every command is in the table")
}

pub fn by_menu_path(path: &str) -> Option<&'static CmdInfo> {
    ALL.iter().find(|i| !i.menu.is_empty() && i.menu == path)
}

/// The text of a menu item: the last segment of its path.
pub fn item_label(i: &CmdInfo) -> &'static str {
    i.menu.rsplit(" > ").next().unwrap_or(i.menu)
}

/// One entry of a menu, in order. A three-level path (`View > Panels > Browser`) becomes an item inside a submenu.
pub enum MenuEntry {
    Separator,
    Item(&'static CmdInfo),
    /// A submenu made from the commands whose middle path part is this name.
    Sub(&'static str, Vec<&'static CmdInfo>),
    /// A submenu the app builds itself (Recent, Bookmarks, Layers).
    Special(&'static str),
}

/// Where the specials go: (command id, before the command?, name).
const SPECIALS: [(&str, bool, &str); 3] = [("quick-open", false, "Recent"), ("manage-layouts", false, "Layers"), ("add-bookmark", true, "Bookmarks")];

/// The entries of one top-level menu, built from the table: groups are separated, submenus gather their items.
pub fn menu_entries(top: &str) -> Vec<MenuEntry> {
    let prefix = format!("{top} > ");
    let mut out: Vec<MenuEntry> = Vec::new();
    let mut last_group: Option<u8> = None;
    let push_special = |out: &mut Vec<MenuEntry>, id: &str, before: bool, last_group: &mut Option<u8>, group: u8| {
        for (sid, sbefore, name) in SPECIALS {
            if sid == id && sbefore == before {
                if before && last_group.is_some_and(|g| g != group) {
                    out.push(MenuEntry::Separator);
                    *last_group = Some(group);
                }
                out.push(MenuEntry::Special(name));
            }
        }
    };
    for i in ALL.iter().filter(|i| i.menu.starts_with(&prefix)) {
        push_special(&mut out, i.id, true, &mut last_group, i.group);
        if last_group.is_some_and(|g| g != i.group) {
            out.push(MenuEntry::Separator);
        }
        last_group = Some(i.group);
        let parts: Vec<&str> = i.menu.split(" > ").collect();
        if parts.len() == 3 {
            let name = parts[1];
            match out.iter_mut().find_map(|e| if let MenuEntry::Sub(n, v) = e { (*n == name).then_some(v) } else { None }) {
                Some(v) => v.push(i),
                None => out.push(MenuEntry::Sub(name, vec![i])),
            }
        } else {
            out.push(MenuEntry::Item(i));
        }
        push_special(&mut out, i.id, false, &mut last_group, i.group);
    }
    out
}

/// Commands that keep working while a text field has the keyboard focus (the rest would type into it).
pub fn works_while_typing(cmd: Cmd) -> bool {
    matches!(cmd, Cmd::OpenFile | Cmd::Save | Cmd::SaveAs | Cmd::Undo | Cmd::Redo | Cmd::QuickOpen | Cmd::Palette | Cmd::GoTo | Cmd::FindInMaps | Cmd::Help)
}

// ------------------------------------------------------------------------------------------------------------------ keys

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeySpec {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: egui::Key,
}

/// `Ctrl+Shift+P`, `F1`, `Del`, `?`, `Ctrl+,`.
pub fn parse_key(s: &str) -> Option<KeySpec> {
    let mut spec = KeySpec { ctrl: false, shift: false, alt: false, key: egui::Key::A };
    let mut rest = s;
    loop {
        if let Some(r) = rest.strip_prefix("Ctrl+") {
            spec.ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Shift+") {
            spec.shift = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("Alt+") {
            spec.alt = true;
            rest = r;
        } else {
            break;
        }
    }
    spec.key = match rest {
        "Del" => egui::Key::Delete,
        "Esc" => egui::Key::Escape,
        "," => egui::Key::Comma,
        "?" => egui::Key::Questionmark,
        other => egui::Key::from_name(other)?,
    };
    Some(spec)
}

impl KeySpec {
    /// Whether a key press with these modifiers is this key. A shortcut with Ctrl or Alt needs exactly those modifiers (and Shift
    /// exactly as written); a plain key (`F`, `G`, `1`) ignores Shift, so Shift+F still reaches the 3D fly mode's `F`.
    pub fn matches(&self, key: egui::Key, m: egui::Modifiers) -> bool {
        if key != self.key || m.command != self.ctrl || m.alt != self.alt {
            return false;
        }
        if self.ctrl || self.alt {
            m.shift == self.shift
        } else {
            !self.shift || m.shift
        }
    }
}

/// The key bindings, parsed once, longest modifier list first (so Ctrl+Shift+S is tried before Ctrl+S).
fn bindings() -> &'static [(Cmd, KeySpec)] {
    static B: std::sync::OnceLock<Vec<(Cmd, KeySpec)>> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let mut v: Vec<(Cmd, KeySpec)> = Vec::new();
        for i in ALL.iter() {
            for k in i.keys {
                if let Some(s) = parse_key(k) {
                    v.push((i.cmd, s));
                }
            }
        }
        v.sort_by_key(|(_, s)| std::cmp::Reverse(s.ctrl as u8 + s.shift as u8 + s.alt as u8));
        v
    })
}

/// Takes the key presses of this frame that are command keys out of the input and returns the commands. While a text field has
/// the focus only [`works_while_typing`] commands are taken. Tab is not taken here: `raw_input_hook` removes it earlier so
/// egui does not use it to move the focus, and the app handles it from there.
pub fn take_pressed(input: &mut egui::InputState, typing: bool) -> Vec<Cmd> {
    let mut out = Vec::new();
    for (cmd, spec) in bindings() {
        if typing && !works_while_typing(*cmd) {
            continue;
        }
        if *cmd == Cmd::Toggle3d {
            continue;
        }
        let at = input.events.iter().position(|e| matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. } if spec.matches(*key, *modifiers)));
        if let Some(i) = at {
            input.events.remove(i);
            out.push(*cmd);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_unique_keys_and_labels() {
        let mut keys = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        let mut ids = std::collections::HashSet::new();
        for i in all() {
            assert!(!i.label.is_empty() && !i.help.is_empty() && !i.id.is_empty(), "{} lacks a label, id or help", i.id);
            assert!(ids.insert(i.id), "duplicate id {}", i.id);
            assert!(labels.insert(i.label), "duplicate label {}", i.label);
            for k in i.keys {
                assert!(parse_key(k).is_some(), "{k} does not parse");
                assert!(keys.insert(k.to_lowercase()), "key {k} is bound twice");
            }
        }
        // every command of the enum is reachable through `info`
        assert_eq!(info(Cmd::Save).keys, &["Ctrl+S"]);
        assert_eq!(info(Cmd::Tool(Tool::Paint)).keys, &["5"]);
        assert_eq!(info(Cmd::HeatCycle).keys, &["H"]);
        assert_eq!(info(Cmd::Rulers).keys, &["Ctrl+R"]);
    }

    #[test]
    fn keys_parse() {
        let s = parse_key("Ctrl+Shift+P").unwrap();
        assert!(s.ctrl && s.shift && !s.alt && s.key == egui::Key::P);
        assert_eq!(parse_key("F1").unwrap().key, egui::Key::F1);
        assert_eq!(parse_key("Del").unwrap().key, egui::Key::Delete);
        assert_eq!(parse_key("Ctrl+,").unwrap().key, egui::Key::Comma);
        assert_eq!(parse_key("?").unwrap().key, egui::Key::Questionmark);
        assert!(parse_key("Ctrl+Alt+S").unwrap().alt);
        assert!(parse_key("Ctrl+Nonsense").is_none());
    }

    #[test]
    fn modifiers_match_the_way_the_editor_needs() {
        use egui::{Key, Modifiers};
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let save = parse_key("Ctrl+S").unwrap();
        let save_as = parse_key("Ctrl+Shift+S").unwrap();
        assert!(save.matches(Key::S, ctrl) && !save.matches(Key::S, ctrl_shift) && !save.matches(Key::S, Modifiers::NONE));
        assert!(save_as.matches(Key::S, ctrl_shift) && !save_as.matches(Key::S, ctrl));
        // a plain key ignores Shift but not Ctrl
        let f = parse_key("F").unwrap();
        assert!(f.matches(Key::F, Modifiers::NONE) && f.matches(Key::F, Modifiers::SHIFT) && !f.matches(Key::F, ctrl));
        let g = parse_key("G").unwrap();
        assert!(!g.matches(Key::G, ctrl), "Ctrl+G is Go to, not Drop to ground");
    }

    #[test]
    fn longer_shortcuts_win_when_keys_are_taken() {
        let mut input = egui::InputState::default();
        input.events.push(egui::Event::Key { key: egui::Key::S, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT });
        input.events.push(egui::Event::Key { key: egui::Key::G, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
        input.events.push(egui::Event::Key { key: egui::Key::G, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        let got = take_pressed(&mut input, false);
        assert_eq!(got.len(), 3);
        assert!(got.contains(&Cmd::SaveAs) && !got.contains(&Cmd::Save));
        assert!(got.contains(&Cmd::GoTo) && got.contains(&Cmd::DropToGround));
        assert!(input.events.is_empty());
        // typing in a text field: only the commands that still work there
        let mut input = egui::InputState::default();
        for (key, modifiers) in [(egui::Key::G, egui::Modifiers::NONE), (egui::Key::P, egui::Modifiers::COMMAND), (egui::Key::Delete, egui::Modifiers::NONE)] {
            input.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
        }
        assert_eq!(take_pressed(&mut input, true), vec![Cmd::QuickOpen]);
        assert_eq!(input.events.len(), 2, "the keys that were not commands stay for the text field");
    }

    #[test]
    fn menu_paths_two_or_three_levels() {
        let mut seen = std::collections::HashSet::new();
        for i in all() {
            if i.menu.is_empty() {
                continue;
            }
            let parts: Vec<&str> = i.menu.split(" > ").collect();
            assert!(parts.len() == 2 || parts.len() == 3, "{} must be <menu> > <item> or <menu> > <submenu> > <item>", i.menu);
            assert!(TOP_MENUS.contains(&parts[0]), "{} is under an unknown menu", i.menu);
            assert!(seen.insert(i.menu), "{} appears twice", i.menu);
            assert_eq!(item_label(i), *parts.last().unwrap());
        }
        // every top-level menu has at least one command
        for t in TOP_MENUS {
            assert!(all().iter().any(|i| i.menu.starts_with(&format!("{t} > "))), "menu {t} is empty");
        }
        assert_eq!(by_menu_path("File > Save").map(|i| i.cmd), Some(Cmd::Save));
        assert_eq!(by_menu_path("View > Panels > Browser").map(|i| i.cmd), Some(Cmd::ShowBrowser));
        assert!(by_menu_path("File > Nonsense").is_none());
    }

    /// The menu tree has the shape of the spec: submenus gather their items, groups are separated, specials sit where they belong.
    #[test]
    fn menu_tree_has_submenus_and_specials() {
        let view = menu_entries("View");
        let subs: Vec<&str> = view.iter().filter_map(|e| if let MenuEntry::Sub(n, _) = e { Some(*n) } else { None }).collect();
        assert_eq!(subs, vec!["Panels", "Layout", "Heatmap"]);
        assert!(view.iter().any(|e| matches!(e, MenuEntry::Sub("Heatmap", v) if v.len() == 3)));
        assert!(view.iter().any(|e| matches!(e, MenuEntry::Item(i) if i.cmd == Cmd::Rulers)));
        assert!(view.iter().any(|e| matches!(e, MenuEntry::Item(i) if i.cmd == Cmd::WorldView)));
        let specials: Vec<&str> = view.iter().filter_map(|e| if let MenuEntry::Special(n) = e { Some(*n) } else { None }).collect();
        assert_eq!(specials, vec!["Layers", "Bookmarks"]);
        let file = menu_entries("File");
        assert!(file.iter().any(|e| matches!(e, MenuEntry::Sub("Export", v) if v.len() == 9)));
        assert!(file.iter().any(|e| matches!(e, MenuEntry::Special("Recent"))));
        let tools = menu_entries("Tools");
        assert!(tools.iter().any(|e| matches!(e, MenuEntry::Sub("Client", v) if v.len() == 5)));
        assert!(tools.iter().any(|e| matches!(e, MenuEntry::Sub("Maps", v) if v.len() == 4)));
        assert!(tools.iter().any(|e| matches!(e, MenuEntry::Sub("Spawns", v) if v.len() == 2)));
        // no two separators in a row, none at the start or the end
        for top in TOP_MENUS {
            let e = menu_entries(top);
            assert!(!matches!(e.first(), Some(MenuEntry::Separator)) && !matches!(e.last(), Some(MenuEntry::Separator)), "{top}");
            assert!(e.windows(2).all(|w| !(matches!(w[0], MenuEntry::Separator) && matches!(w[1], MenuEntry::Separator))), "{top}");
        }
    }
}
