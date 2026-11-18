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
