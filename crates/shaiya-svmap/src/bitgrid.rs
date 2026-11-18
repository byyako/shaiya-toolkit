//! Packed square bit grid: bit `z * size + x`, least significant bit first, 1 = blocked.

/// A `size` x `size` grid of cells. The packed bytes are kept as read (padding bits included) so that a file written
/// back is identical even if its last byte carries stray bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitGrid {
    size: u32,
    bytes: Vec<u8>,
}

/// Number of bytes of the packed grid for a given side (`u32 * u32` always fits `u64`).
pub fn packed_len(size: u32) -> u64 {
    (size as u64 * size as u64).div_ceil(8)
}

/// A run of consecutive cells in `z * size + x` order: `len` cells from cell index `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: u64,
    pub len: u64,
}

impl BitGrid {
    /// All cells free.
    pub fn new(size: u32) -> BitGrid {
        let n = packed_len(size) as usize;
        BitGrid { size, bytes: vec![0; n] }
    }

    /// From packed bytes; `None` when the length is not `ceil(size^2 / 8)`.
    pub fn from_bytes(size: u32, bytes: Vec<u8>) -> Option<BitGrid> {
        if packed_len(size) != bytes.len() as u64 {
            return None;
        }
        Some(BitGrid { size, bytes })
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    fn index(&self, x: u32, z: u32) -> Option<usize> {
        if x < self.size && z < self.size {
            Some(z as usize * self.size as usize + x as usize)
        } else {
            None
        }
    }

    /// Blocked state of a cell; cells outside the grid read as free.
    pub fn get(&self, x: u32, z: u32) -> bool {
        match self.index(x, z) {
            Some(i) => self.bytes[i >> 3] & (1 << (i & 7)) != 0,
            None => false,
        }
    }

    /// Sets a cell; out-of-range cells are ignored.
    pub fn set(&mut self, x: u32, z: u32, value: bool) {
        if let Some(i) = self.index(x, z) {
            if value {
                self.bytes[i >> 3] |= 1 << (i & 7);
            } else {
                self.bytes[i >> 3] &= !(1 << (i & 7));
            }
        }
    }

    /// Fills the cells `x0 <= x < x1`, `z0 <= z < z1` (half-open, clamped to the grid). Returns how many cells changed.
    pub fn fill_rect(&mut self, x0: i64, z0: i64, x1: i64, z1: i64, value: bool) -> usize {
        let s = self.size as i64;
        let (x0, x1) = (x0.clamp(0, s), x1.clamp(0, s));
        let (z0, z1) = (z0.clamp(0, s), z1.clamp(0, s));
        let mut changed = 0;
        for z in z0..z1 {
            for x in x0..x1 {
                if self.get(x as u32, z as u32) != value {
                    self.set(x as u32, z as u32, value);
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Fills the cells whose centre (x + 0.5, z + 0.5) is within `radius` cells of the centre (`cx`, `cz`) given in
    /// cell units; a radius of 0 fills the one cell containing the point. Returns how many cells changed.
    pub fn fill_circle(&mut self, cx: f32, cz: f32, radius: f32, value: bool) -> usize {
        let r = radius.max(0.0);
        let s = self.size as i64;
        let x0 = ((cx - r).floor() as i64 - 1).clamp(0, s);
        let x1 = ((cx + r).ceil() as i64 + 1).clamp(0, s);
        let z0 = ((cz - r).floor() as i64 - 1).clamp(0, s);
        let z1 = ((cz + r).ceil() as i64 + 1).clamp(0, s);
        let (home_x, home_z) = (cx.floor() as i64, cz.floor() as i64);
        let r2 = r * r;
        let mut changed = 0;
        for z in z0..z1 {
            for x in x0..x1 {
                let dx = x as f32 + 0.5 - cx;
                let dz = z as f32 + 0.5 - cz;
                let inside = dx * dx + dz * dz <= r2 || (x == home_x && z == home_z);
                if inside && self.get(x as u32, z as u32) != value {
                    self.set(x as u32, z as u32, value);
                    changed += 1;
                }
            }
        }
        changed
    }

    /// 4-connected flood fill: every cell connected to (`x`, `z`) through cells of the same state as that cell is set
    /// to `value`. Returns the number of cells changed (0 when the start already has `value` or is outside).
    pub fn flood_fill(&mut self, x: u32, z: u32, value: bool) -> usize {
        if self.index(x, z).is_none() || self.get(x, z) == value {
            return 0;
        }
        let from = !value;
        let mut stack = vec![(x, z)];
        self.set(x, z, value);
        let mut changed = 1;
        while let Some((cx, cz)) = stack.pop() {
            let mut visit = |nx: u32, nz: u32, grid: &mut BitGrid, stack: &mut Vec<(u32, u32)>| {
                if grid.index(nx, nz).is_some() && grid.get(nx, nz) == from {
                    grid.set(nx, nz, value);
                    stack.push((nx, nz));
                    changed += 1;
                }
            };
            if cx > 0 {
                visit(cx - 1, cz, self, &mut stack);
            }
            visit(cx + 1, cz, self, &mut stack);
            if cz > 0 {
                visit(cx, cz - 1, self, &mut stack);
            }
            visit(cx, cz + 1, self, &mut stack);
        }
        changed
    }

    /// Fills every cell.
    pub fn fill_all(&mut self, value: bool) {
        let total = self.size as u64 * self.size as u64;
        let full = (total / 8) as usize;
        let rest = (total % 8) as u32;
        let b = if value { 0xFF } else { 0x00 };
        for byte in &mut self.bytes[..full] {
            *byte = b;
        }
        if rest > 0 {
            let mask = (1u8 << rest) - 1;
            let last = &mut self.bytes[full];
            *last = if value { *last | mask } else { *last & !mask };
        }
    }

    /// Number of blocked cells (padding bits past the last cell are not counted).
    pub fn count(&self) -> u64 {
        let total = self.size as u64 * self.size as u64;
        let full = (total / 8) as usize;
        let rest = (total % 8) as u32;
        let mut n: u64 = self.bytes[..full].iter().map(|b| b.count_ones() as u64).sum();
        if rest > 0 {
            n += (self.bytes[full] & ((1u8 << rest) - 1)).count_ones() as u64;
        }
        n
    }

    /// Inverts the cells `x0 <= x < x1`, `z0 <= z < z1` (half-open, clamped to the grid). Returns how many cells were inverted.
    pub fn invert_rect(&mut self, x0: i64, z0: i64, x1: i64, z1: i64) -> usize {
        let s = self.size as i64;
        let (x0, x1) = (x0.clamp(0, s), x1.clamp(0, s));
        let (z0, z1) = (z0.clamp(0, s), z1.clamp(0, s));
        let mut n = 0;
        for z in z0..z1 {
            for x in x0..x1 {
                let v = self.get(x as u32, z as u32);
                self.set(x as u32, z as u32, !v);
                n += 1;
            }
        }
        n
    }

    /// The runs of cells (in `z * size + x` order) whose state differs from `other`; `None` when the sizes differ. Bits past
    /// the last cell are ignored.
    pub fn diff_runs(&self, other: &BitGrid) -> Option<Vec<Run>> {
        if self.size != other.size {
            return None;
        }
        let total = self.size as u64 * self.size as u64;
        let mut runs: Vec<Run> = Vec::new();
        for (bi, (a, b)) in self.bytes.iter().zip(&other.bytes).enumerate() {
            let x = a ^ b;
            if x == 0 {
                continue;
            }
            for bit in 0..8u64 {
                if x & (1 << bit) == 0 {
                    continue;
                }
                let cell = bi as u64 * 8 + bit;
                if cell >= total {
                    break;
                }
                match runs.last_mut() {
                    Some(r) if r.start + r.len == cell => r.len += 1,
                    _ => runs.push(Run { start: cell, len: 1 }),
                }
            }
        }
        Some(runs)
    }

    /// Inverts every cell of the runs (the inverse of itself: applying the same runs twice restores the grid). Cells past the
    /// grid are ignored.
    pub fn flip_runs(&mut self, runs: &[Run]) {
        let total = self.size as u64 * self.size as u64;
        for r in runs {
            let end = (r.start + r.len).min(total);
            let mut c = r.start;
            while c < end {
                // whole bytes at once when the run covers them
                if c & 7 == 0 && c + 8 <= end {
                    self.bytes[(c >> 3) as usize] ^= 0xFF;
                    c += 8;
                } else {
                    self.bytes[(c >> 3) as usize] ^= 1 << (c & 7);
                    c += 1;
                }
            }
        }
    }

    /// Bounding rectangle (x0, z0, x1, z1), half-open, of a set of runs on this grid's side; `None` for no runs.
    pub fn runs_bounds(&self, runs: &[Run]) -> Option<(u32, u32, u32, u32)> {
        let s = self.size as u64;
        if s == 0 {
            return None;
        }
        let mut b: Option<(u64, u64, u64, u64)> = None;
        for r in runs.iter().filter(|r| r.len > 0) {
            let (z0, z1) = (r.start / s, (r.start + r.len - 1) / s);
            let (x0, x1) = if z0 == z1 { (r.start % s, (r.start + r.len - 1) % s + 1) } else { (0, s) };
            b = Some(match b {
                None => (x0, z0, x1, z1 + 1),
                Some(o) => (o.0.min(x0), o.1.min(z0), o.2.max(x1), o.3.max(z1 + 1)),
            });
        }
        b.map(|(a, b, c, d)| (a as u32, b as u32, c as u32, d as u32))
    }

    /// Number of cells that differ from `other` (same size required, else `None`).
    pub fn differing_cells(&self, other: &BitGrid) -> Option<u64> {
        if self.size != other.size {
            return None;
        }
        let total = self.size as u64 * self.size as u64;
        let full = (total / 8) as usize;
        let rest = (total % 8) as u32;
        let mut n: u64 = self.bytes[..full]
            .iter()
            .zip(&other.bytes[..full])
            .map(|(a, b)| (a ^ b).count_ones() as u64)
            .sum();
        if rest > 0 {
            n += ((self.bytes[full] ^ other.bytes[full]) & ((1u8 << rest) - 1)).count_ones() as u64;
        }
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_order_is_lsb_first_z_major() {
        let mut g = BitGrid::new(8);
        g.set(3, 2, true); // bit 2*8+3 = 19 -> byte 2, bit 3
        assert_eq!(g.as_bytes()[2], 1 << 3);
        assert!(g.get(3, 2));
        assert!(!g.get(2, 3));
        g.set(3, 2, false);
        assert_eq!(g.count(), 0);
    }

    #[test]
    fn out_of_range_is_free_and_ignored() {
        let mut g = BitGrid::new(8);
        g.set(8, 0, true);
        g.set(0, 8, true);
        assert_eq!(g.count(), 0);
        assert!(!g.get(100, 100));
    }

    #[test]
    fn from_bytes_checks_length() {
        assert!(BitGrid::from_bytes(8, vec![0; 8]).is_some());
        assert!(BitGrid::from_bytes(8, vec![0; 7]).is_none());
        assert!(BitGrid::from_bytes(3, vec![0; 2]).is_some()); // 9 cells -> 2 bytes
        assert!(BitGrid::from_bytes(3, vec![0; 1]).is_none());
        assert_eq!(BitGrid::from_bytes(0, vec![]).unwrap().count(), 0);
    }

    #[test]
    fn padding_bits_survive_and_are_not_counted() {
        let g = BitGrid::from_bytes(3, vec![0xFF, 0xFF]).unwrap();
        assert_eq!(g.count(), 9);
        assert_eq!(g.to_bytes(), vec![0xFF, 0xFF]);
        let mut h = BitGrid::new(3);
        h.fill_all(true);
        assert_eq!(h.count(), 9);
        assert_eq!(h.differing_cells(&g), Some(0));
    }

    #[test]
    fn rect_fill_clamps_and_counts_changes() {
        let mut g = BitGrid::new(16);
        assert_eq!(g.fill_rect(-5, -5, 4, 4, true), 16);
        assert_eq!(g.fill_rect(2, 2, 6, 6, true), 16 - 4);
        assert_eq!(g.count(), 16 + 12);
        assert_eq!(g.fill_rect(14, 14, 30, 30, true), 4);
        assert_eq!(g.fill_rect(5, 5, 5, 9, true), 0);
        assert_eq!(g.fill_rect(0, 0, 16, 16, false), 32);
    }

    #[test]
    fn circle_fill_is_roughly_a_disc() {
        let mut g = BitGrid::new(64);
        let n = g.fill_circle(32.0, 32.0, 5.0, true);
        assert!((70..=90).contains(&n), "{n}"); // pi * 25 = 78.5
        assert!(g.get(32, 32) && g.get(27, 32) && !g.get(26, 26));
        let mut one = BitGrid::new(8);
        assert_eq!(one.fill_circle(3.5, 4.5, 0.0, true), 1);
        assert!(one.get(3, 4));
        let mut edge = BitGrid::new(8);
        assert!(edge.fill_circle(0.0, 0.0, 3.0, true) > 5); // clamps at the border
    }

    #[test]
    fn flood_fill_is_four_connected() {
        let mut g = BitGrid::new(8);
        for z in 0..8 {
            g.set(4, z, true);
        }
        g.set(4, 4, false); // a one-cell gap
        // the gap joins both sides: 7 * 8 free cells + the gap = 57
        let mut open = g.clone();
        assert_eq!(open.flood_fill(0, 0, true), 57);
        g.set(4, 4, true); // closed wall: only the left side (4 * 8 cells) fills
        assert_eq!(g.flood_fill(0, 0, true), 32);
        assert!(!g.get(5, 0));
        assert_eq!(g.flood_fill(0, 0, true), 0);
        assert_eq!(g.flood_fill(9, 9, true), 0);
        // diagonal-only connection must not leak
        let mut d = BitGrid::new(4);
        d.set(1, 1, true);
        d.set(2, 2, true);
        d.set(1, 2, false);
        assert_eq!(d.flood_fill(1, 1, false), 1);
        assert!(d.get(2, 2));
    }

    #[test]
    fn differing_cells_needs_same_size() {
        let a = BitGrid::new(8);
        let mut b = BitGrid::new(8);
        b.set(1, 1, true);
        assert_eq!(a.differing_cells(&b), Some(1));
        assert_eq!(a.differing_cells(&BitGrid::new(16)), None);
    }

    #[test]
    fn invert_rect_flips_and_clamps() {
        let mut g = BitGrid::new(8);
        g.set(1, 1, true);
        assert_eq!(g.invert_rect(0, 0, 3, 3), 9);
        assert!(!g.get(1, 1) && g.get(0, 0) && g.get(2, 2));
        assert_eq!(g.count(), 8);
        assert_eq!(g.invert_rect(6, 6, 20, 20), 4);
        g.invert_rect(0, 0, 3, 3);
        g.invert_rect(6, 6, 8, 8);
        assert_eq!(g.count(), 1);
    }

    #[test]
    fn diff_and_flip_runs_are_inverse() {
        let a = BitGrid::new(20); // 400 cells = 50 bytes, rows are not byte aligned
        let mut b = a.clone();
        b.fill_rect(3, 2, 17, 9, true);
        b.set(0, 19, true);
        b.set(19, 19, true);
        let runs = a.diff_runs(&b).unwrap();
        // rows 2..9 give one run per row; (19, 2) and (0, 3) are neighbours in index order, so they merge
        let cells: u64 = runs.iter().map(|r| r.len).sum();
        assert_eq!(cells, 14 * 7 + 2);
        assert_eq!(runs.iter().filter(|r| r.len > 1).count(), 6 + 1);
        let mut c = a.clone();
        c.flip_runs(&runs);
        assert_eq!(c, b);
        c.flip_runs(&runs);
        assert_eq!(c, a);
        assert_eq!(a.diff_runs(&a).unwrap(), vec![]);
        assert!(a.diff_runs(&BitGrid::new(8)).is_none());
        // a long run covers whole bytes
        let mut full = BitGrid::new(64);
        let mut other = full.clone();
        other.fill_all(true);
        let runs = full.diff_runs(&other).unwrap();
        assert_eq!(runs, vec![Run { start: 0, len: 4096 }]);
        full.flip_runs(&runs);
        assert_eq!(full, other);
    }

    #[test]
    fn diff_ignores_padding_bits() {
        let a = BitGrid::from_bytes(3, vec![0x00, 0x00]).unwrap();
        let b = BitGrid::from_bytes(3, vec![0x00, 0xFE]).unwrap(); // only bits past cell 8 differ
        assert_eq!(a.diff_runs(&b).unwrap(), vec![]);
        let mut c = a.clone();
        c.flip_runs(&[Run { start: 6, len: 100 }]); // clamps at the 9 cells
        assert_eq!(c.count(), 3);
        assert_eq!(c.as_bytes()[1], 0x01); // only cell 8 flipped; the padding bits stay untouched
    }

    #[test]
    fn runs_bounds_is_the_bounding_rectangle() {
        let g = BitGrid::new(16);
        assert_eq!(g.runs_bounds(&[]), None);
        assert_eq!(g.runs_bounds(&[Run { start: 3 * 16 + 4, len: 5 }]), Some((4, 3, 9, 4)));
        assert_eq!(g.runs_bounds(&[Run { start: 3 * 16 + 14, len: 5 }]), Some((0, 3, 16, 5)));
        assert_eq!(g.runs_bounds(&[Run { start: 16, len: 1 }, Run { start: 5 * 16 + 9, len: 2 }]), Some((0, 1, 11, 6)));
    }
}
