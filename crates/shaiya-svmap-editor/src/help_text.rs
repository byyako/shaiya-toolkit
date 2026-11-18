//! All the explaining text of the editor in one place: tool tips and the status-bar hint per tool, layer tips, tips for the toolbar,
//! the map list, the outliner and the status bar, and what every Problems rule means and how to fix it. The Help window's
//! generated "Problems explained" page and the Problems list's tooltips read the same table, and tests check that every rule the
//! validators can report has a row here.
//!
//! Style (checked by `ui_strings_are_short`): short and plain; a tooltip is one or two sentences of at most 100 characters, a tool
//! hint at most 80; start with the verb or the fact; no "not X because Y" constructions.

use crate::view2d::Tool;

/// The tooltip of a tool in the tool palette (the palette adds the name and the key).
pub fn tool_tip(t: Tool) -> &'static str {
    match t {
        Tool::Select => "Click selects. Drag moves. Shift adds. Alt cycles overlapping records.",
        Tool::Box => "Drag out a monster area, bind area or region. Click for a section B point.",
        Tool::Npc => "Click to place an NPC of the type and id in the toolbar.",
        Tool::Portal => "Click to place a portal. Asks first.",
        Tool::Paint => "Paint blocked cells. Left blocks, right frees. 2D only.",
        Tool::Measure => "Click points to measure a path. Drag for one line. Alt+drag for a radius.",
        Tool::Scatter => "Fill a region with monster areas. Preview first. 2D only.",
        Tool::Place => "Move, turn, add and delete buildings, shapes and trees of the client map.",
        Tool::Terrain => "Raise, lower, flatten, smooth, set and ramp the ground of the client map.",
    }
}

/// One line for the status bar while the cursor is in the viewport.
pub fn tool_hint(t: Tool) -> &'static str {
    match t {
        Tool::Select => "Select: click a record. Drag to move. Drag empty ground to pick several.",
        Tool::Box => "Box: drag to draw a box. Shift snaps 10x. Esc returns to Select.",
        Tool::Npc => "NPC: click to place an NPC. Esc returns to Select.",
        Tool::Portal => "Portal: click to place a portal. Asks first. Esc returns to Select.",
        Tool::Paint => "Paint: left blocks, right frees. [ and ] resize the brush. Esc returns.",
        Tool::Measure => "Measure: click points. Enter ends. Backspace undoes a point. Esc clears.",
        Tool::Scatter => "Scatter: drag or click a region. Enter applies. R reseeds. Esc cancels.",
        Tool::Place => "Placements: click picks. Drag moves. R turns. Del deletes. Esc clears.",
        Tool::Terrain => "Terrain: drag paints. Shift inverts. [ and ] resize. Esc cancels the stroke.",
    }
}

/// The hint line of the 3D view (the status bar shows it).
pub const HINT_3D: &str = "Drag orbits or moves a record. Right drag pans. Wheel zooms. F frames. Tab: 2D.";

pub struct LayerHelp {
    /// The name `--layers` and the settings use.
    pub key: &'static str,
    pub label: &'static str,
    pub tip: &'static str,
}

/// The layers, in the order of the old layers menu (the layers popover groups them).
pub const LAYERS: &[LayerHelp] = &[
    LayerHelp { key: "terrain", label: "terrain shading", tip: "Hillshade of the client's terrain heights, tinted by texture colour." },
    LayerHelp { key: "minimap", label: "client minimap", tip: "The client's minimap picture, laid over the map." },
    LayerHelp { key: "dungeon", label: "dungeon floor", tip: "Top-down picture of a dungeon floor from its collision mesh." },
    LayerHelp { key: "objects", label: "buildings and trees (2D dots)", tip: "The .wld placements as dots. Added or moved ones show even with this off." },
    LayerHelp { key: "buildings", label: "buildings and shapes (3D)", tip: "3D: the buildings and shapes the map places." },
    LayerHelp { key: "trees", label: "trees (3D)", tip: "3D: the trees the map places." },
    LayerHelp { key: "blocked", label: "blocked cells", tip: "Cells monsters cannot walk. Players are not checked." },
    LayerHelp { key: "grid", label: "grid", tip: "A line every 64 units. Every cell from 8 pixels per unit." },
    LayerHelp { key: "ghosts", label: ".wld ghosts", tip: "Grey outlines of the client's .wld: portal boxes, NPC marks, areas, zone groups." },
    LayerHelp { key: "names", label: "labels", tip: "Text labels on records when zoomed in." },
    LayerHelp { key: "areas", label: "monster areas", tip: "Spawn boxes, blue (level 1) to red (level 80) by mean monster level." },
    LayerHelp { key: "npcs", label: "NPCs", tip: "NPC pins with a facing arrow. Extra points are the walking path." },
    LayerHelp { key: "portals", label: "portals", tip: "The ring is the 40-unit use range. The arrow shows a same-map destination." },
    LayerHelp { key: "binds", label: "bind areas", tip: "Respawn areas, coloured by country." },
    LayerHelp { key: "regions", label: "regions", tip: "Named areas. The client shows the name when you walk in." },
    LayerHelp { key: "b", label: "section B", tip: "Section B points. Meaning unknown. Community name: ladders." },
    LayerHelp { key: "a", label: "section A", tip: "Section A rows. Meaning unknown. Empty in every original map." },
    LayerHelp { key: "compare", label: "compare (changes)", tip: "Red dashed removed, green added, amber changed, purple blocked cells." },
];

/// Tips for the controls that are not tools or layers. Keys are the ones the editor passes; an unknown key gives "".
pub fn tip(key: &str) -> &'static str {
    match key {
        "view-2d" => "Top-down editor (Tab).",
        "view-3d" => "3D view of terrain, dungeon and records (Tab).",
        "snap" => "Step that dragged and nudged positions snap to. Shift makes it 10x.",
        "ground-on-move" => "Moved records drop to the ground.",
        "layers" => "Show or hide layers in 2D and 3D.",
        "cull" => "Off shows dungeon ceilings and walls from both sides.",
        "show-3d-stats" => "A box with draw calls and triangle counts in the 3D view.",
        "draw-target" => "What the Box tool draws.",
        "measure-radius" => "Drag a circle with its radius. Alt+drag does the same.",
        "npc-type" => "The type and id of the NPC the NPC tool places.",
        "client-ok" => "The client folder is read in place. Nothing is written to it.",
        "client-none" => "No client folder. Click to add one for names, terrain and checks.",
        "col-id" => "Map number. The file is <number>.svmap.",
        "col-kind" => "F is a field, D a dungeon.",
        "col-size" => "Map side in cells.",
        "col-name" => "The client's name for the map. A dot marks unsaved changes.",
        "map-filter" => "Part of a map number or name. Ctrl+P opens a map directly.",
        "outliner-filter" => "Part of a record's id, name or monster. Ctrl+F jumps here.",
        "outliner-eye" => "Show or hide this section in the viewport.",
        "outliner-lock" => "A locked section cannot be selected or moved in the viewport.",
        "outliner-add" => "Add a record of this section at the view centre.",
        "tab-dot" => "A dot after the name means unsaved changes.",
        "tab-close" => "Close this map. Asks when it has unsaved changes.",
        "status-cursor" => "World position of the cursor (x right, z up). Click to copy x and z.",
        "status-ground" => "Terrain height or dungeon floor under the cursor.",
        "status-cell" => "Cell number (z * size + x) and whether it is blocked.",
        "status-region" => "The named region under the cursor.",
        "status-heat" => "The heatmap tile under the cursor: monsters in it and their mean level.",
        "status-verified" => "The saved file was read again and matches the map.",
        "status-staged" => "This map's .wld comes from the staging folder and overrides the client's copy.",
        "status-wld-readonly" => "The client .wld does not write back byte for byte, so the editor keeps it read-only.",
        "status-client-unsaved" => "The client map has unsaved changes. Ctrl+S writes them to the staging folder.",
        "place-models" => "Open the model list. Click a name, then click the map to add it.",
        "place-yaw" => "The heading in degrees. 0 faces north (+z).",
        "place-select" => "Click a placement to select it. Drag moves it. R turns it.",
        "place-snap" => "Set the height to the terrain under the placement. Fields only.",
        "terrain-brush" => "Raise or lower, flatten, smooth, set a height, or ramp between two points.",
        "terrain-radius" => "How far the brush reaches, in units. [ and ] change it.",
        "terrain-falloff" => "0 is a hard edge. 1 fades from the centre. The white ring marks full strength.",
        "terrain-amount" => "How strongly a held brush works. Shift swaps Raise and Lower.",
        "terrain-height" => "The height Set height drives corners to, from -855 to 455.",
        "terrain-keep" => "Buildings, trees, NPCs and portals in the stroke follow the ground.",
        "terrain-reblock" => "Block or free cells whose slope crosses the angle. Red blocks, green frees.",
        "pair-linked" => "Edited together with the client map. Each change goes to the svmap and the .wld.",
        "pair-unpaired" => "The counts differ from the client map, so edits stay on the svmap. Use Sync to match them.",
        "sync-take-client" => "Take client changes the svmap. Take server changes the client map.",
        "status-problems" => "Errors, warnings and info of this map.",
        "status-frame" => "Frame time of the editor. ui is the time spent building the window.",
        "problems-severity" => "Show or hide findings of this kind.",
        "hint-line" => "A reminder for the active tool. Preferences turns it off.",
        "problems-group" => "Group the findings by rule. A group header offers Fix all when every finding has a fix.",
        "problems-filter" => "Part of a rule code, message, section name or #index.",
        "problems-scope" => "This map, or every map of the folder plus the Server and client group. Unsaved edits count.",
        "problems-fix" => "Apply the fix as one undo step.",
        "compare-against" => "Compare this map with its last save, its original (.bak) or the reference folder.",
        "history" => "The steps of this map. Click one to go back or forward to it.",
        "disk-reload" => "Read the file again and drop your unsaved changes.",
        "disk-keep" => "Keep your version. Saving still asks before it overwrites the file.",
        "find-query" => "Names, or mob:<id>, npc:<t>/<id>, portal:<map>, region:<text>, code:<rule>, lv:<a>-<b>, map:<n>.",
        "find-scope" => "Every map of the folder (open tabs with their edits), or only the open map.",
        "find-view" => "Only records whose centre is inside the part of the map in view.",
        "level-filter" => "Dim monster areas outside a level range. Needs the client's monster levels.",
        "overview" => "A small map of the whole map. Click or drag in it to move the view.",
        "template" => "A saved monster area. With one chosen, the Box tool draws with its entries.",
        "scatter-seed" => "The same seed gives the same layout. Change it for another one.",
        "scatter-add" => "Add a monster. Adding one that is there raises its weight.",
        "scatter-chip" => "A monster of the scatter. The weight sets how often it is picked.",
        "scatter-avoid" => "Skip boxes on blocked cells.",
        "scatter-clear" => "Skip boxes near monster areas that are already on the map.",
        "scatter-two" => "One box in four also holds a second kind.",
        "scatter-template" => "Use this template's monsters. The counts become weights.",
        "scatter-fill" => "Use the selected monster areas, bind areas and regions as the region.",
        "pick-monster" => "Pick a monster from the client list.",
        "bulk-rule" => "where => action. Words are Find's, plus map:1-5,35 and count:2-3.",
        "bulk-high" => "Counts may go above 8. Problems then reports area-count-high.",
        "bulk-save" => "Keep this rule under a name. It stays in the settings.",
        "ini-save" => "Write the file after a copy of the old one. Reads it back to check it.",
        "ini-reload" => "Read the file again and drop your unsaved changes.",
        "ini-add-zone" => "Appends the zone at the end of the file and raises MapCount.",
        "ini-pick" => "Click a point on the map. It becomes map, x, y and z of this key.",
        "ini-remove" => "Remove this key. The zone then uses the default zone's value.",
        "add-name" => "Kept only in the notes file. The editor does not write the game's text tables.",
        "add-look" => "The field map whose textures, water, sky and fog the blank field takes.",
        "share-patch" => "Builds psNNNN.patch from the staged files. The patch is unsigned.",
        "share-folder" => "Copies the staged files to data/ in a folder you pick.",
        "share-target" => "A copy of the client. Never the client you opened, and never your main client.",
        "share-install" => "Applies the staged files to data.sah and data.saf after a backup of the index.",
        "share-uninstall" => "Puts the index back as it was before the first install.",
        "share-loose" => "Copies files whose path the archive lacks (new maps) to data/. Never replaces.",
        "add-minimap" => "Writes a 512 x 512 picture shaded from the heights.",
        _ => "",
    }
}

/// The tip of one Map.ini key in the zone form; "" for a key without one.
pub fn ini_tip(key: &str) -> &'static str {
    match key {
        "MapType" => "F is a field map. D is a dungeon.",
        "MapName" => "Name setting of the map. The default zone has -1.",
        "CreateType" => "How the server makes the map: S, F, P, G, R or H.",
        "CreateTime" => "A time that goes with CreateType. Example: (0, 21, 1).",
        "CreateMinUser" => "Fewest players needed to make the map.",
        "EnterMaxUser" => "Most players that may enter.",
        "ExpireTime" => "How long the made map lasts. The unit is the server's.",
        "WarType" => "Set on some war maps. The original files use P.",
        "RebirthMapPos" => "Where players come back: map, x, y, z. -1 or 0, 0, 0 is ignored.",
        "RebirthMapPos1" => "A second rebirth position: map, x, y, z.",
        "RebirthMapPos2" => "A third rebirth position: map, x, y, z.",
        "WeatherState" | "WeatherRate" | "WeatherPower" | "WeatherDelay" | "WeatherNoneDelay" => "A weather setting. The default zone shows the usual value.",
        _ => "",
    }
}

/// Every key `tip` answers (the string lint walks it).
#[cfg(test)]
pub const TIP_KEYS: &[&str] = &[
    "view-2d", "view-3d", "snap", "ground-on-move", "layers", "cull", "show-3d-stats", "draw-target", "npc-type", "client-ok", "client-none", "col-id", "col-kind", "col-size", "col-name", "map-filter",
    "outliner-filter", "outliner-eye", "outliner-lock", "outliner-add", "tab-dot", "tab-close", "status-cursor", "status-ground", "status-cell", "status-region", "status-heat", "status-verified", "status-staged", "status-wld-readonly", "status-client-unsaved", "pair-linked", "place-models", "place-yaw", "place-select", "place-snap", "terrain-brush", "terrain-radius", "terrain-falloff", "terrain-amount", "terrain-height", "terrain-keep", "terrain-reblock", "pair-unpaired", "sync-take-client", "status-problems", "status-frame",
    "problems-severity", "hint-line", "problems-group", "problems-filter", "problems-scope", "problems-fix", "compare-against", "history", "disk-reload", "disk-keep", "find-query", "find-scope",
    "find-view", "level-filter", "overview", "template", "scatter-seed", "scatter-add", "scatter-chip", "scatter-avoid", "scatter-clear", "scatter-two", "scatter-template", "scatter-fill", "pick-monster", "bulk-rule",
    "bulk-high", "bulk-save", "measure-radius", "ini-save", "ini-reload", "ini-add-zone", "ini-pick", "ini-remove", "add-name", "add-look", "add-minimap",
    "share-patch", "share-folder", "share-target", "share-install", "share-uninstall", "share-loose",
];

/// The Map.ini keys `ini_tip` answers (the string lint walks it).
#[cfg(test)]
pub const INI_KEYS: &[&str] = &["MapType", "MapName", "CreateType", "CreateTime", "CreateMinUser", "EnterMaxUser", "ExpireTime", "WarType", "RebirthMapPos", "RebirthMapPos1", "RebirthMapPos2", "WeatherState", "WeatherRate", "WeatherPower", "WeatherDelay", "WeatherNoneDelay"];

pub struct RuleHelp {
    pub code: &'static str,
    pub severity: &'static str,
    pub title: &'static str,
    pub why: &'static str,
    pub fix: &'static str,
}

const fn r(code: &'static str, severity: &'static str, title: &'static str, why: &'static str, fix: &'static str) -> RuleHelp {
    RuleHelp { code, severity, title, why, fix }
}

/// Every rule the validators can report: `shaiya_svmap::validate::RULE_CODES`, `client_rules::CLIENT_RULE_CODES` and `cross_rules::RULE_CODES`.
pub const RULES: &[RuleHelp] = &[
    r("pos-out-of-range", "error", "Position outside the map", "The server only accepts 0 <= x, z < size (CMap::IsSafePos).", "Move it inside the map, or clamp the corner inside."),
    r("region-out-of-range", "warning", "Region corner past the edge", "A region corner is past the map edge; the server does not use regions any further (4 original regions do this).", "Clamp the corner inside the map, or leave it."),
    r("box-inverted", "error", "Box with min above max", "On one axis the box's min is above its max.", "Swap min and max on that axis."),
    r("non-finite", "error", "NaN or infinity", "A number is NaN or infinite.", "Select the record and type a real value."),
    r("blocked-size", "error", "Blocked grid has the wrong length", "The blocked grid is not size x size cells: the file is damaged.", "Not fixable here; restore the file from a backup."),
    r("area-empty", "info", "Monster area spawns nothing", "The box has no monster entries, so it spawns nothing (61 original areas are like this).", "Pick a monster for it, or delete the area."),
    r("area-count-zero", "error", "Monster count 0", "An entry with count 0.", "Set the count to 1 or more."),
    r("area-count-high", "warning", "Monster count above 8", "The original files use counts 1 to 8.", "Set the count to 8 or less."),
    r("area-mob-zero", "warning", "Monster id 0", "Monster id 0 is the placeholder record \"Error Monster\".", "Pick a real monster."),
    r("area-on-blocked", "warning", "Monster area centre on a blocked cell", "The box centre is on a blocked cell, so monsters there cannot walk.", "Move the area onto free ground, or free the cells."),
    r("npc-no-points", "error", "NPC without points", "An NPC needs at least its standing point.", "Add a point, or delete the NPC."),
    r("npc-type-invalid", "error", "NPC type above 13", "NPC types run 0 to 13.", "Set a valid type."),
    r("npc-type-zero", "info", "NPC type 0", "Type 0 entries are placeholders (58 original entries are like this).", "Nothing to do unless it is a mistake."),
    r("npc-on-blocked", "warning", "NPC on a blocked cell", "The NPC's first point is on a blocked cell.", "Move the NPC, or free the cell."),
    r("portal-levels", "warning", "Portal level range reversed", "Level min is above level max (and max is not 0).", "Swap the two levels."),
    r("portal-country", "warning", "Portal country unknown", "The country is not 0 (any), 1, 2 or an instance id from 1000.", "Set the country to 0, 1 or 2."),
    r("portal-map-missing", "warning", "Destination map not in the folder", "The destination map has no file in the open folder (and is not 200, the guild ranking).", "Pick another destination, or open the folder that has the map."),
    r("portal-dest-outside", "error", "Destination outside its map", "The destination position is outside the destination map.", "Pick the destination again on the destination map."),
    r("portal-dest-blocked", "warning", "Destination on a blocked cell", "The destination is on a blocked cell of that map (4 original portals do this).", "Move the destination to free ground."),
    r("bind-no-box", "error", "Bind area without a box", "A bind area needs a box.", "Add a box, or delete the bind area."),
    r("bind-country", "warning", "Bind country above 3", "Bind countries are 0 to 3.", "Set the country to 0."),
    r("bind-on-blocked", "warning", "Bind box centre on a blocked cell", "A box centre is on a blocked cell.", "Move the box onto free ground."),
    r("mob-unknown", "error", "Unknown monster", "The id is not in the client's Monster.SData, or the record has no name.", "Pick a monster from the list."),
    r("npc-unknown", "error", "Unknown NPC", "The (type, id) pair is not in the client's NpcQuest.SData.", "Pick an NPC from the list."),
    r("size-mismatch", "error", "Map size differs from the client", "The svmap size is not the size of the client's .wld terrain.", "The size cannot change. Use File > New from client map."),
    r("portal-far", "error", "Portal too far from the client's portal", "The server refuses a portal more than 40 units (x, z) from the centre of the client's portal box at the same index.", "Move the portal within 40 units of the grey box (the .wld centre)."),
    r("portal-count", "warning", "Portal count differs from the client", "The client sends the portal index, so the count and order must match the client's .wld.", "Use Tools > Client > Sync from client, or fix the .wld."),
    r("portal-field", "warning", "Portal field differs from the client", "A portal field differs from the .wld (the server uses the svmap value).", "Use Tools > Client > Sync from client to take the .wld value."),
    r("npc-count", "warning", "NPC count differs from the client", "The minimap and quest marks come from the .wld, whose NPC list has another length.", "Use Tools > Client > Sync from client."),
    r("npc-not-in-client", "warning", "NPC not in the client", "This NPC has no counterpart in the client's .wld.", "Use Tools > Client > Sync from client."),
    r("npc-differs", "warning", "NPC differs from the client", "The NPC's type, id, position or angle differs from the .wld.", "Use Tools > Client > Sync from client to take the .wld values."),
    r("region-count", "warning", "Region count differs from the client", "Region names come from the .wld areas by index.", "Use Tools > Client > Sync from client."),
    r("region-differs", "warning", "Region differs from the client", "The region's box or kind differs from the .wld area at the same index.", "Use Tools > Client > Sync from client to take the .wld box."),
    r("bind-count", "info", "Bind area count differs from the client", "Bind areas mirror the .wld zone groups.", "Use Tools > Client > Sync from client."),
    r("bind-differs", "info", "Bind area differs from the client", "The bind area differs from the .wld zone group at the same index.", "Use Tools > Client > Sync from client."),
    r("client-no-wld", "info", "No .wld for this map", "The client has no readable .wld file (world/ plus the map number) for this map, so the client checks were skipped.", "Nothing to do; a map the client lacks cannot be checked."),
    r("wld-not-lossless", "info", "Client map stays read-only", "The client's .wld does not write back byte for byte, so the editor will not change it.", "Nothing to do. Run wld-check to see where it differs."),
    r("portal-dest-no-map", "error", "Destination map does not exist", "The server does not load the destination map, or the client has no .wld for it.", "Pick a map that both Map.ini and the client have."),
    r("mapini-no-svmap", "error", "Map.ini lists a map without a file", "An id below MapCount has no svmap. The original server does not start. Afterglow skips it (info).", "Add the map file, or lower MapCount."),
    r("svmap-not-loaded", "info", "Map file the server does not load", "Neither Map.ini nor MapWar.ini loads this map id.", "Raise MapCount, or ignore the file."),
    r("mapini-rebirth-outside", "error", "Rebirth position not on the map", "A rebirth position is outside its map or on a blocked cell.", "Pick a free position inside the map."),
    r("map-no-client", "warning", "Server map without a client map", "The client has no .wld file for this server map. Players see nothing or crash.", "Add the .wld to the client, or remove the map."),
    r("map-no-name", "info", "Map without a name", "The map id has no name in sysmsg, so the client shows the number.", "Nothing to do, or reuse a named id."),
    r("npc-unknown-server", "error", "NPC not in the server table", "The (type, id) pair is not in the server's NpcQuest.SData. The server skips the NPC.", "Pick an NPC the server table has."),
    r("npc-client-server-differ", "warning", "NPC differs between server and client", "The server and client NpcQuest.SData records differ in model, speed, faction or gate.", "Make both NpcQuest.SData files agree."),
    r("gate-dest-invalid", "error", "Gatekeeper destination invalid", "A gatekeeper destination is on a missing map or outside its map.", "Fix the gate in NpcQuest.SData."),
    r("mob-unknown-server", "error", "Monster not in the server table", "The monster id is not in the Afterglow defs export. Runs only when an export is set.", "Pick a monster the server has."),
    r("wld-ref-missing", "warning", ".wld names a missing file", "The map's .wld names a model, texture, water, sky or sound file the client does not have.", "Add the file to the client, or edit the .wld."),
    r("dg-missing", "error", "Dungeon geometry missing", "The dungeon's .dg file is not in the client, so the map cannot load.", "Add the .dg file to the client."),
    r("minimap-missing", "info", "Field map without a minimap picture", "The client has no minimap .tga for this field map. Most maps have none.", "Nothing to do, or add a 512 x 512 picture."),
    r("obelisk-pos", "warning", "Obelisk not on the map", "An Obelisk.ini position is outside its map or on a blocked cell.", "Move the obelisk to free ground inside the map."),
    r("server-data-note", "info", "Server file skipped", "A server data file is missing or unreadable, so the rules that need it were skipped.", "Check the server data folder in Preferences."),
    r("tactics-zone-map", "info", "Tactics zone on a missing map", "An area of the client's world/TacticsZone.zon names a map that does not exist.", "Nothing to do, or fix the area's map id."),
];

pub fn rule(code: &str) -> Option<&'static RuleHelp> {
    RULES.iter().find(|r| r.code == code)
}

/// The tooltip of a Problems row.
pub fn rule_tip(code: &str) -> String {
    match rule(code) {
        Some(r) => format!("{} ({})\n{}\nFix: {}", r.title, code, r.why, r.fix),
        None => code.to_string(),
    }
}

/// Keys that are not commands (the command table lists the others): (keys, what).
pub const FIXED_KEYS: &[(&str, &str)] = &[
    ("Esc", "cancel the drag or the tool and go back to Select"),
    ("arrow keys (Shift x10)", "nudge the selection by the snap step"),
    ("[ and ]", "Paint-blocked and Terrain: smaller / larger brush"),
    ("Enter (Scatter)", "close the polygon, then add the previewed areas"),
    ("Backspace (Scatter)", "take the last polygon point back"),
    ("R (Scatter)", "pick another seed"),
    ("R (Placements)", "turn the selected placements by 15 degrees (Shift: -15)"),
    ("Shift (Terrain)", "Raise lowers and Lower raises while it is held"),
    ("F (3D view)", "focus the selection"),
    ("Shift+F (3D view)", "fly mode: WASD, Q / E down and up, wheel = speed, Shift = fast"),
    ("N (3D view)", "face north"),
    ("Alt+click", "cycle through overlapping records"),
    ("Shift+click", "add to the selection"),
    ("Right click (no drag)", "context menu of the 2D and 3D views"),
    ("Right or middle drag", "pan the 2D view"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_rules::CLIENT_RULE_CODES;
    use crate::cross_rules::RULE_CODES as CROSS_RULE_CODES;
    use shaiya_svmap::validate::RULE_CODES;

    #[test]
    fn every_rule_has_help() {
        for code in RULE_CODES.iter().chain(CLIENT_RULE_CODES).chain(CROSS_RULE_CODES) {
            let r = rule(code).unwrap_or_else(|| panic!("rule {code} has no help row"));
            assert!(!r.title.is_empty() && !r.why.is_empty() && !r.fix.is_empty(), "{code} has an empty field");
            assert!(["error", "warning", "info"].contains(&r.severity), "{code}: bad severity");
        }
        // and no row for a rule that does not exist
        for r in RULES {
            assert!(RULE_CODES.contains(&r.code) || CLIENT_RULE_CODES.contains(&r.code) || CROSS_RULE_CODES.contains(&r.code), "{} is in the help but nothing reports it", r.code);
        }
        let mut codes: Vec<&str> = RULES.iter().map(|r| r.code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), RULES.len(), "a rule appears twice");
        assert!(rule_tip("portal-far").contains("40 units"));
    }

    #[test]
    fn every_tool_and_layer_has_tip() {
        for t in Tool::ALL {
            assert!(!tool_tip(t).is_empty() && !tool_hint(t).is_empty());
            assert!(tool_hint(t).starts_with(t.label().split('-').next().unwrap_or("")), "{:?}: the hint names its tool", t);
        }
        let mut keys = std::collections::HashSet::new();
        for l in LAYERS {
            assert!(!l.tip.is_empty() && !l.label.is_empty());
            assert!(keys.insert(l.key));
        }
        for key in [
            "view-2d", "view-3d", "snap", "ground-on-move", "layers", "cull", "draw-target", "npc-type", "client-ok", "client-none", "col-id", "col-kind", "col-size", "col-name", "map-filter",
            "outliner-filter", "outliner-eye", "outliner-lock", "outliner-add", "tab-dot", "tab-close", "status-cursor", "status-ground", "status-cell", "status-region", "status-problems",
            "status-frame", "problems-severity", "hint-line", "problems-group", "problems-filter", "problems-scope", "problems-fix", "compare-against", "history", "disk-reload", "disk-keep", "show-3d-stats", "measure-radius",
        ] {
            assert!(!tip(key).is_empty(), "no tip for {key}");
        }
        assert_eq!(tip("nonsense"), "");
    }


    /// The words and phrases the text rules ban in UI strings: contrast constructions and filler.
    pub(crate) fn banned_in(text: &str) -> Option<&'static str> {
        let lower = text.to_lowercase();
        for phrase in [", not ", "rather than", "because", "which means", "note that"] {
            if lower.contains(phrase) {
                return Some(phrase);
            }
        }
        ["please", "simply", "just", "basically"].into_iter().find(|word| lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == *word))
    }

    /// Every tooltip, hint, rule text and command help is short and plain (the limits and banned words of the text style).
    #[test]
    fn ui_strings_are_short() {
        let mut bad: Vec<String> = Vec::new();
        let mut check = |what: &str, text: &str, max: usize| {
            if text.chars().count() > max {
                bad.push(format!("{what}: {} chars (max {max}): {text}", text.chars().count()));
            }
            if let Some(w) = banned_in(text) {
                bad.push(format!("{what}: has \"{w}\": {text}"));
            }
        };
        for t in Tool::ALL {
            check(&format!("tool tip {}", t.label()), tool_tip(t), 100);
            check(&format!("tool hint {}", t.label()), tool_hint(t), 80);
        }
        check("3D hint", HINT_3D, 80);
        for l in LAYERS {
            check(&format!("layer {}", l.key), l.tip, 100);
        }
        for k in super::TIP_KEYS {
            check(&format!("tip {k}"), tip(k), 100);
        }
        for k in super::INI_KEYS {
            check(&format!("ini key {k}"), ini_tip(k), 100);
            assert!(!ini_tip(k).is_empty(), "{k}");
        }
        for r in RULES {
            check(&format!("rule {} title", r.code), r.title, 40);
            check(&format!("rule {} why", r.code), r.why, 120);
            check(&format!("rule {} fix", r.code), r.fix, 70);
        }
        for i in crate::commands::all() {
            check(&format!("command {} help", i.id), i.help, 70);
        }
        for (n, t) in crate::inspector::TIPS.iter().enumerate() {
            check(&format!("inspector tip {n}"), t, 100);
        }
        for (n, t) in crate::inspector::TEXTS.iter().enumerate() {
            check(&format!("inspector text {n}"), t, 100);
        }
        for (n, t) in crate::prefs::TIPS.iter().enumerate() {
            check(&format!("prefs tip {n}"), t, 100);
        }
        for (k, what) in FIXED_KEYS {
            check(&format!("key {k}"), what, 100);
        }
        assert!(bad.is_empty(), "{} strings break the text style:\n{}", bad.len(), bad.join("\n"));
    }
    #[test]
    fn every_layer_key_flips_a_setting() {
        for l in LAYERS {
            let mut on = crate::settings::Settings::default();
            let mut off = on.clone();
            crate::app::set_layer(&mut on, l.key, true);
            crate::app::set_layer(&mut off, l.key, false);
            assert_ne!(on.layers, off.layers, "layer key {} does nothing", l.key);
        }
    }
}
