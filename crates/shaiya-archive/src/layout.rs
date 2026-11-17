//! Where the entries lie in `data.saf`: gaps, overlaps, shared ranges, out-of-bounds entries. Index only; no SAF reads.

use crate::Index;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Layout {
    pub saf_len: Option<u64>,
    pub data_end: u64,
    /// Union of `[offset, offset + length)` over every file entry (the same rule as `compact::usage`).
    pub live_bytes: u64,
    /// `saf_len - live_bytes`.
    pub dead_bytes: Option<u64>,
    pub gap_count: usize,
    /// `(offset, bytes)`, at most 20, largest first.
    pub largest_gaps: Vec<(u64, u64)>,
    /// Entries starting before the end of an earlier one (the number of overlapping pairs).
    pub overlaps: usize,
    /// Of those, entries with exactly the same `(offset, length)` as another.
    pub shared_ranges: usize,
    /// Display paths, at most 100 (the count is in `out_of_bounds_count`).
    pub out_of_bounds: Vec<String>,
    pub out_of_bounds_count: usize,
    pub zero_length: usize,
}

/// Zero-length entries are skipped for gaps and overlaps (as the audit script does). Gaps include the space before the first entry and,
/// when `saf_len` is known, after the last one.
pub fn analyse(index: &Index, saf_len: Option<u64>) -> Layout {
    let all = index.entries();
    let mut l = Layout { saf_len, data_end: index.data_end(), ..Layout::default() };
    let mut ranges: Vec<(u64, u64)> = Vec::with_capacity(all.len());
    for f in &all {
        let e = f.entry;
        if e.length == 0 {
            l.zero_length += 1;
        }
        let end = e.offset.saturating_add(e.length as u64);
        if saf_len.is_some_and(|n| end > n) {
            l.out_of_bounds_count += 1;
            if l.out_of_bounds.len() < 100 {
                l.out_of_bounds.push(f.path.clone());
            }
        }
        if e.length > 0 {
            ranges.push((e.offset, end));
        }
    }
    ranges.sort_unstable_by_key(|&(a, b)| (a, b - a));
    let mut gaps: Vec<(u64, u64)> = Vec::new();
    let (mut cursor, mut used) = (0u64, 0u64);
    let mut prev: Option<(u64, u64)> = None;
    for &(start, end) in &ranges {
        if start < cursor {
            l.overlaps += 1;
            if prev == Some((start, end)) {
                l.shared_ranges += 1;
            }
        }
        if start > cursor {
            gaps.push((cursor, start - cursor));
        }
        used += end.saturating_sub(start.max(cursor));
        cursor = cursor.max(end);
        prev = Some((start, end));
    }
    if let Some(n) = saf_len {
        if cursor < n {
            gaps.push((cursor, n - cursor));
        }
        l.dead_bytes = Some(n.saturating_sub(used));
    }
    l.live_bytes = used;
    l.gap_count = gaps.len();
    gaps.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    gaps.truncate(20);
    l.largest_gaps = gaps;
    l
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dir, Entry};

    fn en(n: &str, offset: u64, length: u32) -> Entry {
        Entry { name: n.as_bytes().to_vec(), offset, length, version: 0 }
    }

    fn index(files: Vec<Entry>) -> Index {
        let mut i = Index::new();
        i.root.dirs.push(Dir { name: b"d".to_vec(), files, dirs: vec![] });
        i
    }

    #[test]
    fn hand_computed_layout() {
        // 0..4 a, gap 4..10, 10..20 b, 15..18 c (inside b), 10..20 d (the same range as b), 20..20 zero, 30..40 e (past a gap of 10), then
        // saf is 50 bytes: trailing gap 40..50
        let idx = index(vec![en("a", 0, 4), en("b", 10, 10), en("c", 15, 3), en("d", 10, 10), en("z", 20, 0), en("e", 30, 10)]);
        let l = analyse(&idx, Some(50));
        assert_eq!(l.live_bytes, 4 + 10 + 10);
        assert_eq!(l.dead_bytes, Some(26));
        assert_eq!(l.gap_count, 3); // 4..10 (6), 20..30 (10), 40..50 (10)
        assert_eq!(l.largest_gaps, vec![(20, 10), (40, 10), (4, 6)]);
        assert_eq!(l.overlaps, 2, "c inside b, d on b");
        assert_eq!(l.shared_ranges, 1, "d shares b's exact range");
        assert_eq!(l.zero_length, 1);
        assert_eq!(l.data_end, 40);
        assert_eq!(l.out_of_bounds_count, 0);
        // saf unknown: no trailing gap and no dead bytes
        let l = analyse(&idx, None);
        assert_eq!((l.dead_bytes, l.gap_count), (None, 2));
        // a smaller saf puts entries out of bounds
        let l = analyse(&idx, Some(35));
        assert_eq!(l.out_of_bounds, vec!["d/e"]);
        assert_eq!(l.out_of_bounds_count, 1);
    }

    #[test]
    fn offsets_above_4_gib_do_not_wrap() {
        let big = 5u64 << 30;
        let idx = index(vec![en("a", big, 100), en("b", 0, 10)]);
        let l = analyse(&idx, Some(big + 100));
        assert_eq!(l.data_end, big + 100);
        assert_eq!(l.live_bytes, 110);
        assert_eq!(l.gap_count, 1);
        assert_eq!(l.largest_gaps, vec![(10, big - 10)]);
        assert_eq!(l.dead_bytes, Some(big - 10));
    }

    #[test]
    fn an_empty_index_has_one_gap_when_the_saf_has_bytes() {
        let l = analyse(&Index::new(), Some(7));
        assert_eq!((l.live_bytes, l.dead_bytes, l.gap_count), (0, Some(7), 1));
        let l = analyse(&Index::new(), Some(0));
        assert_eq!((l.gap_count, l.dead_bytes), (0, Some(0)));
    }
}
