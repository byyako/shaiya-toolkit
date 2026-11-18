//! Semantic colours for every piece of UI text and icon. Each one has a dark and a light value; the code asks for the name (error,
//! warning, link, ...) and never writes a colour literal. Viewport colours (the level ramp, blocked cells, section colours, the
//! compare overlay on the map) are not here: they stay the same in both themes.

use egui::{Color32, Visuals};

/// The colour names of the UI for the active theme (`overlay_bg` and `compare_cells` are for the map painters).
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Pal {
    pub text: Color32,
    pub weak: Color32,
    pub link: Color32,
    pub ok: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub info: Color32,
    pub selection: Color32,
    pub hover: Color32,
    pub rail_bg: Color32,
    pub overlay_bg: Color32,
    pub overlay_text: Color32,
    pub compare_add: Color32,
    pub compare_remove: Color32,
    pub compare_change: Color32,
    pub compare_cells: Color32,
}

/// The bar that says a file changed on disk: the same dark yellow in both themes, with light text on it.
pub const BANNER_BG: Color32 = Color32::from_rgb(110, 90, 15);

/// The colour of a finding of this severity.
pub fn severity(p: &Pal, s: shaiya_svmap::Severity) -> Color32 {
    match s {
        shaiya_svmap::Severity::Error => p.error,
        shaiya_svmap::Severity::Warning => p.warning,
        shaiya_svmap::Severity::Info => p.info,
    }
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The palette for these visuals (`dark` picks the column of the table). The ten shared colours come from the toolkit's theme.
pub fn pal_from(dark: bool, v: &Visuals) -> Pal {
    let b = shaiya_ui::theme::pal_from(dark, v);
    let overlay_bg = Color32::from_black_alpha(153);
    let overlay_text = rgb(0xe8e8e8);
    let (compare_add, compare_remove, compare_change, compare_cells) = if dark {
        (rgb(0x6edc82), rgb(0xff7878), rgb(0xffc85a), rgb(0xb48cff))
    } else {
        (rgb(0x2e7d32), rgb(0xb3261e), rgb(0x8a5300), rgb(0x6a3fb0))
    };
    Pal {
        text: b.text,
        weak: b.weak,
        link: b.link,
        ok: b.ok,
        error: b.error,
        warning: b.warning,
        info: b.info,
        selection: b.selection,
        hover: b.hover,
        rail_bg: b.rail_bg,
        overlay_bg,
        overlay_text,
        compare_add,
        compare_remove,
        compare_change,
        compare_cells,
    }
}

/// The palette of the context's current theme.
pub fn pal(ctx: &egui::Context) -> Pal {
    let s = ctx.global_style();
    pal_from(s.visuals.dark_mode, &s.visuals)
}

/// The palette for a `Ui` (the same as [`pal`]).
pub fn pal_ui(ui: &egui::Ui) -> Pal {
    pal_from(ui.visuals().dark_mode, ui.visuals())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_ui::theme::contrast;

    #[test]
    fn contrast_math() {
        assert!((contrast(Color32::WHITE, Color32::BLACK) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 0.001);
    }

    /// The text colours read on the panel fill in both themes (WCAG 4.5), and the icons (text colour) on the rail (3.0).
    #[test]
    fn theme_contrast() {
        for dark in [true, false] {
            let v = if dark { Visuals::dark() } else { Visuals::light() };
            let p = pal_from(dark, &v);
            let bg = v.panel_fill;
            for (name, c) in [("text", p.text), ("weak", p.weak), ("link", p.link), ("ok", p.ok), ("error", p.error), ("warning", p.warning), ("info", p.info)] {
                let r = contrast(c, bg);
                assert!(r >= 4.5, "{name} on the panel in the {} theme: {r:.2}", if dark { "dark" } else { "light" });
            }
            let r = contrast(p.text, p.rail_bg);
            assert!(r >= 3.0, "icons on the rail: {r:.2}");
            // the labels drawn over the map use their own backplate in both themes
            let backed = Color32::from_rgb(0x30, 0x30, 0x30);
            assert!(contrast(p.overlay_text, backed) >= 4.5);
        }
    }
}
