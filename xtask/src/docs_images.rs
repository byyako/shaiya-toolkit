//! Keeps the documentation free of broken pictures: every relative `![alt](path)` in `README.md` and `docs/**/*.md` must name a tracked
//! file. Fenced code blocks and inline code are skipped, as the site hook does.

use std::collections::BTreeSet;

/// The relative image targets of one Markdown text, outside fenced code blocks and inline code.
fn image_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    for line in text.lines() {
        let t = line.trim_start();
        let mark = if t.starts_with("```") { Some('`') } else if t.starts_with("~~~") { Some('~') } else { None };
        if let Some(m) = mark {
            match fence {
                None => fence = Some(m),
                Some(open) if open == m => fence = None,
                _ => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        // Inline code spans are dropped (odd segments between backticks).
        let plain: String = line.split('`').enumerate().filter(|(i, _)| i % 2 == 0).map(|(_, s)| s).collect::<Vec<_>>().join(" ");
        let mut rest = plain.as_str();
        while let Some(at) = rest.find("![") {
            rest = &rest[at + 2..];
            let Some(close) = rest.find("](") else { break };
            let after = &rest[close + 2..];
            let Some(end) = after.find(')') else { break };
            let url = after[..end].trim().trim_start_matches('<');
            let url = url.split_whitespace().next().unwrap_or("").trim_end_matches('>');
            let url = url.split('#').next().unwrap_or("");
            let absolute = url.is_empty() || url.starts_with('/') || url.contains("://") || url.starts_with("data:") || url.starts_with("mailto:");
            if !absolute {
                out.push(url.to_string());
            }
            rest = &after[end..];
        }
    }
    out
}

/// Joins `target` onto the folder of `page` (both with `/`), resolving `.` and `..`. `None` when it climbs above the repository.
fn resolve(page: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = page.split('/').collect();
    parts.pop();
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// One line per picture that does not name a tracked file. `pages` is `(path, text)` for each Markdown page.
pub fn missing_images(pages: &[(String, String)], tracked: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for (page, text) in pages {
        for target in image_targets(text) {
            match resolve(page, &target) {
                Some(p) if tracked.contains(&p) => {}
                _ => out.push(format!("{page}: the picture {target} is not a tracked file")),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::root;
    use std::process::Command;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_missing_picture_is_reported_and_a_tracked_one_is_not() {
        let tracked = set(&["docs/assets/a.png"]);
        let pages = vec![("docs/tools/x.md".to_string(), "![ok](../assets/a.png)\n\n![gone](../assets/b.png)\n".to_string())];
        let found = missing_images(&pages, &tracked);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("b.png"));
    }

    #[test]
    fn code_blocks_inline_code_and_web_pictures_are_skipped() {
        let text = "```\n![x](nope.png)\n```\n`![y](nope.png)` and ![z](https://example.org/a.png) and ![w](/abs.png)\n~~~\n![v](nope.png)\n~~~\n";
        let pages = vec![("README.md".to_string(), text.to_string())];
        assert!(missing_images(&pages, &set(&[])).is_empty());
    }

    #[test]
    fn a_picture_climbing_out_of_the_repository_is_reported() {
        let pages = vec![("README.md".to_string(), "![a](../x.png)".to_string())];
        assert_eq!(missing_images(&pages, &set(&["x.png"])).len(), 1);
    }

    #[test]
    fn every_markdown_image_exists() {
        let root = root();
        let out = Command::new("git").args(["-c", "safe.directory=*", "ls-files", "-z"]).current_dir(&root).output().expect("git runs");
        assert!(out.status.success(), "git ls-files failed");
        let tracked: BTreeSet<String> = String::from_utf8_lossy(&out.stdout).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
        let pages: Vec<(String, String)> = tracked
            .iter()
            .filter(|f| f.as_str() == "README.md" || (f.starts_with("docs/") && f.ends_with(".md")))
            .filter_map(|f| std::fs::read_to_string(root.join(f)).ok().map(|t| (f.clone(), t)))
            .collect();
        assert!(pages.len() > 10, "found only {} pages", pages.len());
        let missing = missing_images(&pages, &tracked);
        assert!(missing.is_empty(), "broken pictures:\n{}", missing.join("\n"));
    }
}
