//! Keeps the formats overview complete: every page in `docs/formats/` (the overview itself aside) is linked from `docs/formats/README.md`.

/// The page names of `pages` that the overview text does not link (`](name)` or `](name#anchor)`), in the order given.
fn unlisted(overview: &str, pages: &[String]) -> Vec<String> {
    pages.iter().filter(|p| !overview.contains(&format!("]({p})")) && !overview.contains(&format!("]({p}#"))).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::root;

    #[test]
    fn a_page_the_overview_does_not_link_is_reported() {
        let text = "| [A](a.md) | x |\n| [B](b.md#top) | y |\n";
        let pages: Vec<String> = ["a.md", "b.md", "c.md"].iter().map(|s| s.to_string()).collect();
        assert_eq!(unlisted(text, &pages), ["c.md"]);
        assert!(unlisted("", &[]).is_empty());
    }

    #[test]
    fn every_format_page_is_listed() {
        let dir = root().join("docs").join("formats");
        let overview = std::fs::read_to_string(dir.join("README.md")).expect("docs/formats/README.md exists");
        let mut pages: Vec<String> = std::fs::read_dir(&dir)
            .expect("docs/formats exists")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".md") && n != "README.md")
            .collect();
        pages.sort();
        assert!(pages.len() > 10, "found only {} format pages", pages.len());
        let missing = unlisted(&overview, &pages);
        assert!(missing.is_empty(), "docs/formats/README.md does not link: {}", missing.join(", "));
    }
}
