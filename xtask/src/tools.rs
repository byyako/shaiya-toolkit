//! The programs of the toolkit. Every new tool adds one entry here.

#[allow(dead_code)]
pub struct Tool {
    pub package: &'static str,
    pub bin: &'static str,
    pub asset: &'static str,
    pub title: &'static str,
    pub gui: bool,
    pub bundle_id: Option<&'static str>,
}

pub const TOOLS: &[Tool] = &[
    Tool { package: "shaiya-cli", bin: "shaiya", asset: "shaiya", title: "Shaiya command line", gui: false, bundle_id: None },
    Tool {
        package: "shaiya-toolkit",
        bin: "shaiya-toolkit",
        asset: "shaiya-toolkit",
        title: "Shaiya Toolkit",
        gui: true,
        bundle_id: Some("io.github.byyako.shaiya-toolkit"),
    },
    Tool {
        package: "shaiya-svmap-editor",
        bin: "shaiya-svmap-editor",
        asset: "shaiya-svmap-editor",
        title: "Shaiya SVMap Editor",
        gui: true,
        bundle_id: Some("io.github.byyako.shaiya-svmap-editor"),
    },
];

#[cfg(test)]
mod tests {
    use super::TOOLS;

    #[test]
    fn gui_programs_match_tools() {
        let src = include_str!("../../crates/shaiya-ui/src/lib.rs");
        let a = src.find("// GUI_PROGRAMS begin").expect("begin marker");
        let b = src.find("// GUI_PROGRAMS end").expect("end marker");
        let block = &src[a..b];
        let mut found: Vec<(String, String)> = Vec::new();
        let mut rest = block;
        while let Some(i) = rest.find("(\"") {
            rest = &rest[i + 2..];
            let e1 = rest.find('"').unwrap();
            let bin = rest[..e1].to_string();
            let after = &rest[e1 + 1..];
            let q = after.find('"').expect("title");
            let after = &after[q + 1..];
            let e2 = after.find('"').unwrap();
            found.push((bin, after[..e2].to_string()));
            rest = &after[e2 + 1..];
        }
        let want: Vec<(String, String)> = TOOLS.iter().filter(|t| t.gui).map(|t| (t.bin.to_string(), t.title.to_string())).collect();
        assert_eq!(found, want);
    }
}
