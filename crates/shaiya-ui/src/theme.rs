//! Semantic colours for every piece of UI text. Each has a dark and a light value; the code asks for the name (error, warning, link, ...)
//! and never writes a colour literal. Copied from the svmap editor's `theme.rs` without the map-specific colours.

use egui::{Color32, Visuals};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Dark, Theme::Light, Theme::System];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::System => "System",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Theme::Dark => egui::ThemePreference::Dark,
            Theme::Light => egui::ThemePreference::Light,
            Theme::System => egui::ThemePreference::System,
        }
    }
}

/// The colour names of the UI for the active theme.
#[derive(Clone, Copy, Debug, PartialEq)]
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
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The palette for these visuals (`dark` picks the column of the table).
pub fn pal_from(dark: bool, v: &Visuals) -> Pal {
    if dark {
        Pal {
            text: v.text_color(),
            weak: rgb(0xa0a0a0),
            link: rgb(0x6eaaff),
            ok: rgb(0x6fcf73),
            error: rgb(0xff6b6b),
            warning: rgb(0xffb347),
            info: rgb(0xa0a0a0),
            selection: v.selection.bg_fill,
            hover: v.widgets.hovered.bg_fill,
            rail_bg: v.panel_fill,
        }
    } else {
        Pal {
            text: v.text_color(),
            weak: rgb(0x5f5f5f),
            link: rgb(0x1a5fb4),
            ok: rgb(0x2e7d32),
            error: rgb(0xb3261e),
            warning: rgb(0x8a5300),
            info: rgb(0x5f5f5f),
            selection: v.selection.bg_fill,
            hover: v.widgets.hovered.bg_fill,
            rail_bg: v.panel_fill,
        }
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

fn channel(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio of two opaque colours (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(contrast(p.text, p.rail_bg) >= 3.0, "icons on the rail");
        }
    }

    #[test]
    fn theme_labels_and_serde() {
        assert_eq!(serde_json::to_string(&Theme::System).unwrap(), "\"system\"");
        assert_eq!(serde_json::from_str::<Theme>("\"light\"").unwrap(), Theme::Light);
        assert_eq!(Theme::ALL.map(Theme::label), ["Dark", "Light", "System"]);
    }
}
