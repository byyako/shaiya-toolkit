//! CJK font fallback. No CJK font is bundled (it would add 16 MB or more to every GUI); a system font is probed at start and loaded on a
//! background thread. `SHAIYA_TOOLKIT_FONT` and the `extra_font` setting name one explicitly.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

/// System fonts that cover Hangul and CJK, in order of preference (collection index 0).
pub fn system_candidates(os: crate::settings::Os) -> Vec<PathBuf> {
    use crate::settings::Os;
    let list: &[&str] = match os {
        Os::Windows => &["C:\\Windows\\Fonts\\malgun.ttf", "C:\\Windows\\Fonts\\msyh.ttc", "C:\\Windows\\Fonts\\simsun.ttc"],
        Os::Mac => &["/System/Library/Fonts/AppleSDGothicNeo.ttc", "/System/Library/Fonts/PingFang.ttc", "/Library/Fonts/Arial Unicode.ttf"],
        Os::Linux => &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        ],
    };
    list.iter().map(PathBuf::from).collect()
}

/// The fonts to try, in order: `SHAIYA_TOOLKIT_FONT`, the `extra_font` setting, then the system list.
pub fn candidates(env_font: Option<&str>, extra_font: Option<&str>, os: crate::settings::Os) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for p in [env_font, extra_font].into_iter().flatten() {
        if !p.trim().is_empty() {
            v.push(PathBuf::from(p));
        }
    }
    v.extend(system_candidates(os));
    v
}

/// A font that was found and read.
pub struct LoadedFont {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

/// Reads the first existing candidate on a background thread; the result arrives on the channel (`None` when no candidate exists).
pub fn load_in_background(candidates: Vec<PathBuf>) -> Receiver<Option<LoadedFont>> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new().name("font-probe".into()).spawn(move || {
        let found = candidates.into_iter().find(|p| p.is_file()).and_then(|path| std::fs::read(&path).ok().map(|bytes| LoadedFont { path, bytes }));
        let _ = tx.send(found);
    });
    rx
}

/// egui font definitions with `bytes` appended as a fallback to both the proportional and the monospace family.
pub fn definitions_with(bytes: Vec<u8>) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("shaiya-cjk".to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("shaiya-cjk".to_owned());
    }
    fonts
}

/// True when `s` has Hangul or CJK characters (then the tool asks the shell for the font hint).
pub fn needs_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Os;

    #[test]
    fn candidate_order_puts_explicit_choices_first() {
        let c = candidates(Some("/env/font.ttf"), Some("/setting/font.ttf"), Os::Linux);
        assert_eq!(c[0], PathBuf::from("/env/font.ttf"));
        assert_eq!(c[1], PathBuf::from("/setting/font.ttf"));
        assert_eq!(c.len(), 2 + 5);
        assert_eq!(candidates(None, Some(" "), Os::Windows).len(), 3);
        assert!(system_candidates(Os::Mac)[0].ends_with("AppleSDGothicNeo.ttc"));
    }

    #[test]
    fn hangul_and_cjk_are_detected() {
        assert!(needs_cjk("\u{BAAC}\u{C2A4}\u{D130}"));
        assert!(needs_cjk("a \u{4E2D}\u{6587}"));
        assert!(needs_cjk("\u{30AB}\u{30BF}"));
        assert!(!needs_cjk("Item.SData caf\u{E9} \u{43F}\u{440}\u{438}"));
    }

    #[test]
    fn a_missing_font_gives_none() {
        let rx = load_in_background(vec![PathBuf::from("/no/such/font.ttf")]);
        assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap().is_none());
        let d = std::env::temp_dir().join(format!("shaiya-ui-font-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("f.ttf");
        std::fs::write(&f, b"not really a font").unwrap();
        let got = load_in_background(vec![PathBuf::from("/no/such"), f.clone()]).recv_timeout(std::time::Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!((got.path, got.bytes), (f, b"not really a font".to_vec()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
