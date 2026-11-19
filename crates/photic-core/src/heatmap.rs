//! Column-oriented heatmap with column LOD (order-book liquidity, spectrograms,
//! any `x × y` intensity matrix where x grows over time).
//!
//! Column `c` covers x in `x0 + c .. x0 + c + 1` and holds `rows` intensities
//! for y buckets `y0 + r * dy`. Level `k` merges columns by **max** into
//! buckets aligned to the *global* x grid (`[j * 2^k, (j + 1) * 2^k)`), the same
//! grid [`crate::Pyramid`] uses, so heat columns line up with candles at every
//! zoom level, and a liquidity wall that lasted one column is still visible
//! when zoomed out.

use crate::{Rect, Rgba};

#[derive(Clone, Debug)]
pub struct Heatmap {
    rows: usize,
    /// x of column 0's left edge (an integer, e.g. a bar index).
    x0: usize,
    /// y of the bottom edge of row 0.
    pub y0: f64,
    /// Height of one row in data units.
    pub dy: f64,
    /// `levels[k]` is column-major; stored node `i` is global bucket
    /// `(x0 >> k) + i`, covering x in `[((x0 >> k) + i) << k, .. + 2^k)`.
    levels: Vec<Vec<f32>>,
    max: f32,
}

impl Heatmap {
    pub fn new(rows: usize, x0: usize, y0: f64, dy: f64) -> Self {
        assert!(rows > 0, "heatmap needs at least one row");
        Self {
            rows,
            x0,
            y0,
            dy,
            levels: vec![Vec::new()],
            max: 0.0,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn x0(&self) -> usize {
        self.x0
    }

    /// Number of base columns.
    pub fn cols(&self) -> usize {
        self.levels[0].len() / self.rows
    }

    pub fn max_level(&self) -> usize {
        self.levels.len() - 1
    }

    /// Largest finite intensity seen (for normalising colour).
    pub fn max_value(&self) -> f32 {
        self.max
    }

    fn nodes(&self, k: usize) -> usize {
        self.levels[k].len() / self.rows
    }

    /// Stored node `i` of level `k`.
    pub fn column(&self, k: usize, i: usize) -> &[f32] {
        let l = &self.levels[k.min(self.max_level())];
        &l[i * self.rows..(i + 1) * self.rows]
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
        // global bucket index of the updated node at the current level
        let mut g = self.x0 + self.cols() - 1;
        let mut k = 1;
        while self.nodes(k - 1) > 1 {
            if k == self.levels.len() {
                self.levels.push(Vec::new());
            }
            let off = self.x0 >> k;
            let j = g >> 1;
            // a level is created lazily, so earlier buckets may be missing
            while self.nodes(k) < j - off {
                let node = self.merge_children(k, off + self.nodes(k));
                self.levels[k].extend_from_slice(&node);
            }
            let node = self.merge_children(k, j);
            let i = j - off;
            let parent = &mut self.levels[k];
            if (i + 1) * rows <= parent.len() {
                parent[i * rows..(i + 1) * rows].copy_from_slice(&node);
            } else {
                parent.extend_from_slice(&node);
            }
            g = j;
            k += 1;
        }
    }

    /// Max of the (up to two) level `k - 1` children of global bucket `j`.
    fn merge_children(&self, k: usize, j: usize) -> Vec<f32> {
        let rows = self.rows;
        let child_off = self.x0 >> (k - 1);
        let mut node = vec![0.0f32; rows];
        for c in [2 * j, 2 * j + 1] {
            if c >= child_off && c - child_off < self.nodes(k - 1) {
                let i = c - child_off;
                let child = &self.levels[k - 1][i * rows..(i + 1) * rows];
                for (a, b) in node.iter_mut().zip(child) {
                    if b.is_finite() {
                        *a = a.max(*b);
                    }
                }
            }
        }
        node
    }

    /// Lower visible cells into rects.
    ///
    /// * `level`: column LOD (each emitted column covers `2^level` x units,
    ///   aligned to the global grid like candles)
    /// * `x_range`: visible data x range
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
        let cols = self.cols();
        if cols == 0 || self.max <= 0.0 {
            return 0;
        }
        let k = level.min(self.max_level());
        let off = self.x0 >> k;
        let nodes = self.nodes(k);
        let data_end = (self.x0 + cols) as f64;
        let gx0 = x_range.0.max(self.x0 as f64).floor() as usize;
        let gx1 = x_range.1.min(data_end).ceil().max(0.0) as usize;
        if gx1 <= gx0 {
            return 0;
        }
        let j0 = (gx0 >> k).max(off);
        let j1 = gx1.div_ceil(1 << k).min(off + nodes);
        let row_step = row_step.max(1);
        let r0 =
            (((y_range.0 - self.y0) / self.dy).floor().max(0.0) as usize) / row_step * row_step;
        let r1 = (((y_range.1 - self.y0) / self.dy).ceil().max(0.0) as usize).min(self.rows);
        let inv = 1.0 / self.max;
        let before = out.len();
        for j in j0..j1 {
            let col = self.column(k, j - off);
            // clip partial buckets to the data's own extent
            let xa = ((j << k) as f64).max(self.x0 as f64);
            let xb = (((j + 1) << k) as f64).min(data_end);
            let (x_lo, x_hi) = ((xa - origin.0) as f32, (xb - origin.0) as f32);
            let mut r = r0;
            while r < r1 {
                let end = (r + row_step).min(self.rows);
                let v = col[r..end]
                    .iter()
                    .fold(0.0f32, |m, x| if x.is_finite() { m.max(*x) } else { m });
                let t = (v * inv).max(0.0).sqrt();
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

    /// Reference: max over a global bucket computed directly from base data.
    fn bucket_max(base: &[Vec<f32>], x0: usize, k: usize, j: usize, r: usize) -> f32 {
        ((j << k)..((j + 1) << k))
            .filter(|g| *g >= x0 && g - x0 < base.len())
            .map(|g| base[g - x0][r])
            .fold(0.0, f32::max)
    }

    #[test]
    fn levels_match_globally_aligned_reference() {
        for x0 in [0usize, 1, 5, 13, 1000] {
            for n in [1usize, 2, 3, 7, 37, 64] {
                let rows = 3;
                let base: Vec<Vec<f32>> = (0..n)
                    .map(|c| (0..rows).map(|r| ((c * 7 + r * 3) % 11) as f32).collect())
                    .collect();
                let mut h = Heatmap::new(rows, x0, 0.0, 1.0);
                for col in &base {
                    h.push_column(col);
                }
                assert_eq!(h.cols(), n);
                for k in 0..=h.max_level() {
                    let off = x0 >> k;
                    for i in 0..h.nodes(k) {
                        for r in 0..rows {
                            assert_eq!(
                                h.column(k, i)[r],
                                bucket_max(&base, x0, k, off + i, r),
                                "x0={x0} n={n} k={k} i={i} r={r}"
                            );
                        }
                    }
                }
                assert_eq!(h.nodes(h.max_level()), 1, "x0={x0} n={n}: one top node");
            }
        }
    }

    #[test]
    fn single_wall_survives_zoom_out() {
        let mut h = Heatmap::new(4, 3, 0.0, 1.0);
        for c in 0..37 {
            let v = if c == 21 { 99.0 } else { (c % 5) as f32 };
            h.push_column(&[v, 1.0, 0.0, v * 0.5]);
        }
        assert_eq!(h.max_value(), 99.0);
        assert_eq!(h.column(h.max_level(), 0)[0], 99.0);
    }

    #[test]
    fn cells_respect_threshold_ranges_and_offsets() {
        let mut h = Heatmap::new(10, 0, 100.0, 0.5);
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

        // offset heatmap starting at x = 1001: coarse buckets are aligned to the
        // global grid and clipped to the data, never drawn outside it
        let mut g = Heatmap::new(1, 1001, 0.0, 1.0);
        for _ in 0..4 {
            g.push_column(&[1.0]);
        }
        out.clear();
        let n = g.cells(
            0,
            (990.0, 1003.0),
            (0.0, 1.0),
            1,
            (1000.0, 0.0),
            0.0,
            heat,
            &mut out,
        );
        assert_eq!(n, 2);
        assert_eq!(out[0].min[0], 1.0);
        out.clear();
        g.cells(
            2,
            (0.0, 2000.0),
            (0.0, 1.0),
            1,
            (1000.0, 0.0),
            0.0,
            heat,
            &mut out,
        );
        assert_eq!(
            out[0].min[0], 1.0,
            "bucket [1000,1004) clipped to x0 = 1001"
        );
        assert_eq!(out.last().unwrap().max[0], 5.0, "clipped to data end 1005");
    }
}
