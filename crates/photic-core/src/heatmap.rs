//! Column-oriented heatmap with column LOD (order-book liquidity, spectrograms,
//! any `x × y` intensity matrix where x grows over time).
//!
//! Column `c` holds `rows` intensities for y buckets `y0 + r * dy`. Level `k`
//! merges `2^k` adjacent columns by **max**, so a liquidity wall that existed
//! for a single column is still visible when zoomed out.

use crate::{Rect, Rgba};

#[derive(Clone, Debug)]
pub struct Heatmap {
    rows: usize,
    /// y of the bottom edge of row 0.
    pub y0: f64,
    /// Height of one row in data units.
    pub dy: f64,
    /// x (data units) of column 0's left edge; one column is 1 unit wide.
    pub x0: f64,
    /// `levels[k]` is column-major: column `c` is `[c*rows .. (c+1)*rows]`.
    levels: Vec<Vec<f32>>,
    max: f32,
}

impl Heatmap {
    pub fn new(rows: usize, x0: f64, y0: f64, dy: f64) -> Self {
        assert!(rows > 0, "heatmap needs at least one row");
        Self {
            rows,
            y0,
            dy,
            x0,
            levels: vec![Vec::new()],
            max: 0.0,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Number of base columns.
    pub fn cols(&self) -> usize {
        self.levels[0].len() / self.rows
    }

    pub fn max_level(&self) -> usize {
        self.levels.len() - 1
    }

    /// Largest intensity seen (for normalising colour).
    pub fn max_value(&self) -> f32 {
        self.max
    }

    /// Column `c` of level `k`.
    pub fn column(&self, k: usize, c: usize) -> &[f32] {
        let l = &self.levels[k.min(self.max_level())];
        &l[c * self.rows..(c + 1) * self.rows]
    }

    /// Append one column (streaming). `O(rows · log cols)`.
    pub fn push_column(&mut self, col: &[f32]) {
        assert_eq!(col.len(), self.rows, "column length must equal rows");
        for v in col {
            if v.is_finite() {
                self.max = self.max.max(*v);
            }
        }
        self.levels[0].extend_from_slice(col);
        let rows = self.rows;
        let mut k = 0;
        while self.levels[k].len() / rows > 1 {
            let n = self.levels[k].len() / rows;
            let idx = (n - 1) / 2;
            let mut node = self.levels[k][2 * idx * rows..(2 * idx + 1) * rows].to_vec();
            if 2 * idx + 1 < n {
                let right = &self.levels[k][(2 * idx + 1) * rows..(2 * idx + 2) * rows];
                for (a, b) in node.iter_mut().zip(right) {
                    *a = a.max(*b);
                }
            }
            if k + 1 == self.levels.len() {
                self.levels.push(Vec::new());
            }
            let parent = &mut self.levels[k + 1];
            if (idx + 1) * rows <= parent.len() {
                parent[idx * rows..(idx + 1) * rows].copy_from_slice(&node);
            } else {
                parent.extend_from_slice(&node);
            }
            k += 1;
        }
    }

    /// Lower visible cells into rects.
    ///
    /// * `level`: column LOD (each emitted column covers `2^level` base columns)
    /// * `x_range`: visible data x range (column `c` spans `x0 + c .. x0 + c + 1`)
    /// * `y_range`: visible data y range (rows outside are skipped)
    /// * `row_step`: merge this many rows per emitted cell (max), `>= 1`
    /// * `threshold`: skip cells whose normalised intensity is below this.
    ///   Intensity is `sqrt(v / max)` so mid-level values stay visible next
    ///   to a few huge outliers (liquidity walls).
    #[allow(clippy::too_many_arguments)]
    pub fn cells(
        &self,
        level: usize,
        x_range: (f64, f64),
        y_range: (f64, f64),
        row_step: usize,
        origin: (f64, f64),
        threshold: f32,
        colormap: impl Fn(f32) -> Rgba,
        out: &mut Vec<Rect>,
    ) -> usize {
        let n = self.cols();
        if n == 0 || self.max <= 0.0 {
            return 0;
        }
        let level = level.min(self.max_level());
        let slot = 1usize << level;
        let lvl_cols = self.levels[level].len() / self.rows;
        let first = (x_range.0 - self.x0).floor().max(0.0) as usize;
        let last = ((x_range.1 - self.x0).ceil().max(0.0) as usize).min(n);
        let c0 = (first / slot).min(lvl_cols);
        let c1 = last.div_ceil(slot).min(lvl_cols);
        let row_step = row_step.max(1);
        let r0 =
            (((y_range.0 - self.y0) / self.dy).floor().max(0.0) as usize) / row_step * row_step;
        let r1 = (((y_range.1 - self.y0) / self.dy).ceil().max(0.0) as usize).min(self.rows);
        let inv = 1.0 / self.max;
        let before = out.len();
        for c in c0..c1 {
            let col = self.column(level, c);
            let x_lo = (self.x0 + (c * slot) as f64 - origin.0) as f32;
            let x_hi = (self.x0 + ((c + 1) * slot) as f64 - origin.0) as f32;
            let mut r = r0;
            while r < r1 {
                let end = (r + row_step).min(self.rows);
                let v = col[r..end]
                    .iter()
                    .fold(0.0f32, |m, x| if x.is_finite() { m.max(*x) } else { m });
                let t = (v * inv).sqrt();
                if t >= threshold {
                    let y_lo = (self.y0 + r as f64 * self.dy - origin.1) as f32;
                    let y_hi = (self.y0 + end as f64 * self.dy - origin.1) as f32;
                    out.push(Rect::new([x_lo, y_lo], [x_hi, y_hi], colormap(t)));
                }
                r = end;
            }
        }
        out.len() - before
    }
}

/// "Heat" colour ramp for `t` in 0..1: transparent navy → blue → cyan →
/// yellow → white. Alpha rises with intensity so the chart shows through.
pub fn heat(t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [10.0, 20.0, 60.0]),
        (0.25, [20.0, 70.0, 200.0]),
        (0.5, [0.0, 200.0, 220.0]),
        (0.75, [250.0, 220.0, 40.0]),
        (1.0, [255.0, 255.0, 255.0]),
    ];
    let i = STOPS.iter().position(|s| s.0 >= t).unwrap_or(4).max(1);
    let (a, b) = (STOPS[i - 1], STOPS[i]);
    let f = (t - a.0) / (b.0 - a.0);
    let c = |j: usize| (a.1[j] + (b.1[j] - a.1[j]) * f) as u8;
    Rgba::rgba(c(0), c(1), c(2), (60.0 + 195.0 * t.sqrt()) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_lod_keeps_max_and_matches_rebuild() {
        let rows = 4;
        let mut h = Heatmap::new(rows, 0.0, 0.0, 1.0);
        for c in 0..37 {
            let v = if c == 21 { 99.0 } else { (c % 5) as f32 };
            h.push_column(&[v, 1.0, 0.0, v * 0.5]);
        }
        assert_eq!(h.cols(), 37);
        assert_eq!(h.max_value(), 99.0);
        // top level covers everything, so its max must be the wall
        let top = h.column(h.max_level(), 0);
        assert_eq!(top[0], 99.0);
        // level 3 column containing c=21 (21/8 = 2)
        assert_eq!(h.column(3, 2)[0], 99.0);
    }

    #[test]
    fn cells_respect_threshold_and_ranges() {
        let mut h = Heatmap::new(10, 0.0, 100.0, 0.5);
        for _ in 0..8 {
            let mut col = vec![0.0; 10];
            col[3] = 10.0;
            h.push_column(&col);
        }
        let mut out = Vec::new();
        let n = h.cells(
            0,
            (0.0, 8.0),
            (100.0, 105.0),
            1,
            (0.0, 100.0),
            0.9,
            heat,
            &mut out,
        );
        assert_eq!(n, 8);
        assert_eq!(out[0].min, [0.0, 1.5]);
        assert_eq!(out[0].max, [1.0, 2.0]);
        // y range excluding row 3 -> nothing
        out.clear();
        assert_eq!(
            h.cells(
                0,
                (0.0, 8.0),
                (103.0, 105.0),
                1,
                (0.0, 100.0),
                0.5,
                heat,
                &mut out
            ),
            0
        );
        // offset heatmap: columns start at x = 1000
        let mut g = Heatmap::new(1, 1000.0, 0.0, 1.0);
        for _ in 0..4 {
            g.push_column(&[1.0]);
        }
        out.clear();
        assert_eq!(
            g.cells(
                0,
                (990.0, 1002.0),
                (0.0, 1.0),
                1,
                (1000.0, 0.0),
                0.0,
                heat,
                &mut out
            ),
            2
        );
        assert_eq!(out[0].min[0], 0.0);
    }
}
