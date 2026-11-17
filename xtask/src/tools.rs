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
        package: "shaiya-workbench",
        bin: "shaiya-workbench",
        asset: "shaiya-workbench",
        title: "Shaiya Workbench",
        gui: true,
        bundle_id: Some("io.github.byyako.shaiya-workbench"),
    },
];
