//! One glob syntax for every toolkit command and tab (spec D8), case-insensitive:
//!
//! - `*` and `?` stay inside a path segment, `**` as a whole segment crosses segments (and matches zero of them);
//! - `[a-z]`, `[!x]` character classes;
//! - a pattern without `/` matches the file name in any folder; with a `/` it is anchored at the root of the path;
//! - a trailing `/` means "everything under that folder";
//! - `\` is read as `/` in patterns and paths. No regular expressions.

use crate::error::{Error, Result};
use crate::text::parse_size;

#[derive(Clone, Debug)]
enum Tok {
    Lit(char),
    One,
    Many,
    Class { negate: bool, items: Vec<(char, char)> },
}

#[derive(Clone, Debug)]
enum Seg {
    /// `**`: zero or more whole segments.
    Deep,
    Toks(Vec<Tok>),
}

#[derive(Clone, Debug)]
pub struct Pattern {
    text: String,
    segs: Vec<Seg>,
    /// Matches against the last segment only (the pattern had no `/`).
    name_only: bool,
}

fn lower(c: char) -> char {
    let mut it = c.to_lowercase();
    match (it.next(), it.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

fn parse_segment(seg: &str, whole: &str) -> Result<Seg> {
    if seg == "**" {
        return Ok(Seg::Deep);
    }
    let chars: Vec<char> = seg.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                // consecutive stars inside a segment are one star
                if !matches!(toks.last(), Some(Tok::Many)) {
                    toks.push(Tok::Many);
                }
                i += 1;
            }
            '?' => {
                toks.push(Tok::One);
                i += 1;
            }
            '[' => {
                let mut j = i + 1;
                let negate = matches!(chars.get(j), Some('!') | Some('^'));
                if negate {
                    j += 1;
                }
                let mut items = Vec::new();
                let mut first = true;
                loop {
                    let Some(&c) = chars.get(j) else {
                        return Err(Error::format("pattern", Some(i as u64), format!("unclosed [ in pattern {whole:?}")));
                    };
                    if c == ']' && !first {
                        j += 1;
                        break;
                    }
                    first = false;
                    if chars.get(j + 1) == Some(&'-') && chars.get(j + 2).is_some_and(|&e| e != ']') {
                        let (a, b) = (lower(c), lower(chars[j + 2]));
                        items.push((a.min(b), a.max(b)));
                        j += 3;
                    } else {
                        let c = lower(c);
                        items.push((c, c));
                        j += 1;
                    }
                }
                toks.push(Tok::Class { negate, items });
                i = j;
            }
            c => {
                toks.push(Tok::Lit(lower(c)));
                i += 1;
            }
        }
    }
    Ok(Seg::Toks(toks))
}

fn match_toks(toks: &[Tok], s: &[char]) -> bool {
    match toks.split_first() {
        None => s.is_empty(),
        Some((Tok::Many, rest)) => (0..=s.len()).any(|k| match_toks(rest, &s[k..])),
        Some((Tok::One, rest)) => !s.is_empty() && match_toks(rest, &s[1..]),
        Some((Tok::Lit(c), rest)) => s.first().is_some_and(|x| x == c) && match_toks(rest, &s[1..]),
        Some((Tok::Class { negate, items }, rest)) => match s.first() {
            Some(x) => items.iter().any(|&(a, b)| a <= *x && *x <= b) != *negate && match_toks(rest, &s[1..]),
            None => false,
        },
    }
}

fn match_segs(segs: &[Seg], parts: &[Vec<char>]) -> bool {
    match segs.split_first() {
        None => parts.is_empty(),
        Some((Seg::Deep, rest)) => (0..=parts.len()).any(|k| match_segs(rest, &parts[k..])),
        Some((Seg::Toks(t), rest)) => parts.first().is_some_and(|p| match_toks(t, p)) && match_segs(rest, &parts[1..]),
    }
}

impl Pattern {
    /// Syntax of D8. `Error::Format` for an unclosed class or an empty pattern.
    pub fn new(text: &str) -> Result<Pattern> {
        let norm = text.trim().replace('\\', "/");
        if norm.is_empty() || norm == "/" {
            return Err(Error::format("pattern", None, "empty pattern"));
        }
        let trailing = norm.ends_with('/');
        let body = norm.trim_start_matches('/');
        let has_slash = body.contains('/');
        let mut segs: Vec<Seg> = Vec::new();
        for s in body.split('/').filter(|s| !s.is_empty()) {
            segs.push(parse_segment(s, text)?);
        }
        if segs.is_empty() {
            return Err(Error::format("pattern", None, "empty pattern"));
        }
        if trailing {
            segs.push(Seg::Deep);
            // "Item/" = everything under Item, at least one segment
            segs.insert(segs.len() - 1, Seg::Toks(vec![Tok::Many]));
        }
        Ok(Pattern { text: text.to_string(), segs, name_only: !has_slash && !trailing })
    }

    /// `path` uses `/` (a `\` in either is treated as `/`); case-insensitive (Unicode simple lower case).
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<Vec<char>> = path.split(['/', '\\']).filter(|s| !s.is_empty()).map(|s| s.chars().map(lower).collect()).collect();
        if parts.is_empty() {
            return false;
        }
        if self.name_only {
            let last = std::slice::from_ref(parts.last().expect("non-empty"));
            return match_segs(&self.segs, last);
        }
        match_segs(&self.segs, &parts)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// Include/exclude patterns and a size range.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub include: Vec<Pattern>,
    pub exclude: Vec<Pattern>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
}

impl Filter {
    /// Empty include = everything; a path matches when any include and no exclude matches and the size is in range.
    pub fn matches(&self, path: &str, size: u64) -> bool {
        if self.min_size.is_some_and(|m| size < m) || self.max_size.is_some_and(|m| size > m) {
            return false;
        }
        if !self.include.is_empty() && !self.include.iter().any(|p| p.matches(path)) {
            return false;
        }
        !self.exclude.iter().any(|p| p.matches(path))
    }

    pub fn from_args(include: &[String], exclude: &[String], min_size: Option<&str>, max_size: Option<&str>) -> Result<Filter> {
        let pats = |v: &[String]| v.iter().map(|s| Pattern::new(s)).collect::<Result<Vec<_>>>();
        Ok(Filter {
            include: pats(include)?,
            exclude: pats(exclude)?,
            min_size: min_size.map(parse_size).transpose()?,
            max_size: max_size.map(parse_size).transpose()?,
        })
    }

    /// True when nothing is restricted.
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty() && self.min_size.is_none() && self.max_size.is_none()
    }
}

/// Path lists (`remove.txt`, `--list` files): the first tab-separated column, trimmed, `\` -> `/`; blank lines, `#` lines and a `path`
/// header are skipped (so `docs/cleanup-candidates.tsv` can be used as it is).
pub fn parse_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('\t').next().unwrap_or("").trim().replace('\\', "/"))
        .filter(|p| !p.is_empty() && !p.starts_with('#') && !p.eq_ignore_ascii_case("path"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pat: &str, path: &str) -> bool {
        Pattern::new(pat).unwrap().matches(path)
    }

    #[test]
    fn the_glob_table() {
        let yes: &[(&str, &str)] = &[
            // name only, any folder
            ("*.SData", "Item/Item.SData"),
            ("*.sdata", "Item/ITEM.SDATA"),
            ("*.sdata", "Item.SData"),
            ("Item.SData", "a/b/c/Item.SData"),
            ("*", "anything.txt"),
            ("*", "a/b.txt"),
            ("?.txt", "a/b.txt"),
            ("item.*", "x/Item.dds"),
            ("*.d?s", "x/y.dds"),
            ("*a*b*", "xaxbx"),
            ("a*", "a"),
            // anchored patterns
            ("Item/Item.SData", "item/item.sdata"),
            ("Character/*/*.3DC", "Character/Human/body.3dc"),
            ("Character/*/*.3dc", "CHARACTER/HUMAN/BODY.3DC"),
            ("item/*", "Item/Item.SData"),
            ("*/*.dds", "interface/a.dds"),
            // ** crosses segments, including zero
            ("**/*.dds", "a.dds"),
            ("**/*.dds", "a/b/c/d.dds"),
            ("a/**/z.txt", "a/z.txt"),
            ("a/**/z.txt", "a/b/c/z.txt"),
            ("a/**", "a/b/c"),
            ("**", "x/y"),
            // trailing slash = everything under
            ("Item/", "Item/Item.SData"),
            ("Item/", "item/3do/a.3do"),
            ("Item/3DO/", "Item/3DO/x/y.3do"),
            // classes
            ("[a-c].txt", "b.txt"),
            ("[!a-c].txt", "d.txt"),
            ("x[0-9][0-9].dds", "dir/x42.dds"),
            ("[abc]", "b"),
            ("[]]", "]"),
            ("[a-]", "-"),
            // backslashes
            ("Item\\*.SData", "item/Item.SData"),
            ("item/*.sdata", "Item\\Item.SData"),
            // Korean and non-ASCII case folding
            ("*\u{C6D0}.3do", "Item/3DO/10051\u{C6D0}.3do"),
            ("\u{C6D0}*", "x/\u{C6D0}\u{BC14}.txt"),
            ("\u{E9}*", "x/\u{C9}cole.txt"),
        ];
        for (p, s) in yes {
            assert!(m(p, s), "{p:?} should match {s:?}");
        }
        let no: &[(&str, &str)] = &[
            ("*.sdata", "Item/Item.txt"),
            ("*.sdata", "Item.SData.bak"),
            ("*", ""),
            ("a*", "ba"),
            ("?", "ab"),
            ("a/*.txt", "a/b/c.txt"),
            ("*/*.dds", "a/b/c.dds"),
            ("Character/*/*.3dc", "Character/body.3dc"),
            ("Item/Item.SData", "x/Item/Item.SData"),
            ("Item/", "Item"),
            ("Item/", "Items/a"),
            ("item/*", "item"),
            ("[a-c].txt", "d.txt"),
            ("[!a-c].txt", "b.txt"),
            ("x[0-9]", "xa"),
            ("a/**/z.txt", "b/z.txt"),
        ];
        for (p, s) in no {
            assert!(!m(p, s), "{p:?} should not match {s:?}");
        }
        assert!(yes.len() + no.len() >= 40);
    }

    #[test]
    fn bad_patterns_are_format_errors() {
        assert!(matches!(Pattern::new("[abc"), Err(Error::Format { .. })));
        assert!(matches!(Pattern::new("a[!"), Err(Error::Format { .. })));
        assert!(matches!(Pattern::new(""), Err(Error::Format { .. })));
        assert!(matches!(Pattern::new("   "), Err(Error::Format { .. })));
        assert_eq!(Pattern::new("*.dds").unwrap().as_str(), "*.dds");
    }

    #[test]
    fn filter_combines_include_exclude_and_size() {
        let f = Filter::from_args(&["*.dds".into(), "*.tga".into()], &["interface/**".into()], Some("1KB"), Some("1MiB")).unwrap();
        assert!(f.matches("a/b.dds", 2000));
        assert!(f.matches("b.TGA", 1000));
        assert!(!f.matches("a/b.dds", 999), "below the minimum");
        assert!(!f.matches("a/b.dds", 1 << 21), "above the maximum");
        assert!(!f.matches("interface/b.dds", 2000), "excluded");
        assert!(!f.matches("a/b.wav", 2000), "not included");
        assert!(Filter::default().matches("anything", 0));
        assert!(Filter::default().is_empty());
        assert!(Filter::from_args(&["[x".into()], &[], None, None).is_err());
        assert!(Filter::from_args(&[], &[], Some("lots"), None).is_err());
    }

    #[test]
    fn path_lists_skip_headers_comments_and_blank_lines() {
        let text = "path\tbytes\n# test files\nTEST/junk.eft\t4\n\n  Effect\\never.eft  \n";
        assert_eq!(parse_list(text), vec!["TEST/junk.eft", "Effect/never.eft"]);
        assert!(parse_list("").is_empty());
        assert_eq!(parse_list("PATH\na/b\r\nc/d\r\n"), vec!["a/b", "c/d"]);
    }
}
