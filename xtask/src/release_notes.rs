//! `cargo xtask release-notes <version>`: the `## [<version>]` section of `CHANGELOG.md`; an error when it is missing or empty.

use crate::meta::root;

/// The body of the `## [version]` section (without the heading line), trimmed.
pub fn section(changelog: &str, version: &str) -> Result<String, String> {
    let changelog = changelog.replace("\r\n", "\n");
    let heading = format!("## [{version}]");
    let mut lines = changelog.lines();
    let found = lines.by_ref().any(|l| l.trim_start().starts_with(&heading));
    if !found {
        return Err(format!("CHANGELOG.md has no section \"{heading}\""));
    }
    let body: Vec<&str> = lines.take_while(|l| !l.starts_with("## [")).collect();
    let text = body.join("\n").trim().to_string();
    if text.is_empty() {
        return Err(format!("the CHANGELOG.md section \"{heading}\" is empty"));
    }
    Ok(text)
}

pub fn run(version: &str) -> Result<(), String> {
    let path = root().join("CHANGELOG.md");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{}", section(&text, version.trim_start_matches('v'))?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\n## [Unreleased]\n\n- later\n\n## [0.2.0] - 2027-01-01\n\n### shaiya (CLI)\n- a\n\n## [0.1.0] - 2026-12-01\n\n- first\n- second\n\n## [0.0.1]\n\n";

    #[test]
    fn sections_are_cut_at_the_next_heading() {
        assert_eq!(section(LOG, "0.2.0").unwrap(), "### shaiya (CLI)\n- a");
        assert_eq!(section(LOG, "0.1.0").unwrap(), "- first\n- second");
        assert_eq!(section(LOG, "Unreleased").unwrap(), "- later");
    }

    #[test]
    fn missing_and_empty_sections_are_errors() {
        assert!(section(LOG, "9.9.9").unwrap_err().contains("no section"));
        assert!(section(LOG, "0.0.1").unwrap_err().contains("empty"));
        assert!(section("", "0.1.0").is_err());
    }
}
