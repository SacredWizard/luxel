//! # photic-finance
//!
//! Financial charting on top of `photic-core`: OHLCV bars with an LOD
//! pyramid that keeps true extremes, candle/volume lowering to GPU primitives,
//! and streaming-friendly indicators.

use photic_core::{Aggregate, Heatmap, MinMax, Pyramid, Rect, Rgba, Segment, prims::polyline};

/// One OHLCV bar. `time` is a Unix timestamp in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Bar {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Bar {
    pub fn new(time: i64, open: f64, high: f64, low: f64, close: f64, volume: f64) -> Self {
        Self {
            time,
            open,
            high,
            low,
            close,
            volume,
        }
    }
}

impl Aggregate for Bar {
    /// Coarser candle: first open, last close, true high/low, summed volume.
    #[inline]
    fn merge(&self, next: &Self) -> Self {
        Bar {
            time: self.time,
            open: self.open,
            high: self.high.max(next.high),
            low: self.low.min(next.low),
            close: next.close,
            volume: self.volume + next.volume,
        }
    }
}

/// Bars addressed by index (x = bar index), so session gaps never show as
/// empty space. Use [`OhlcSeries::time_at`] to label the axis.
#[derive(Clone, Debug, Default)]
pub struct OhlcSeries {
    pub pyramid: Pyramid<Bar>,
}

impl OhlcSeries {
    pub fn new(bars: Vec<Bar>) -> Self {
        Self {
            pyramid: Pyramid::build(bars),
        }
    }
    pub fn len(&self) -> usize {
        self.pyramid.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pyramid.is_empty()
    }
    pub fn bars(&self) -> &[Bar] {
        self.pyramid.base()
    }
    pub fn push(&mut self, b: Bar) {
        self.pyramid.push(b)
    }
    pub fn update_last(&mut self, b: Bar) {
        self.pyramid.update_last(b)
    }
    pub fn time_at(&self, idx: usize) -> Option<i64> {
        self.bars().get(idx).map(|b| b.time)
    }
    /// (lowest low, highest high) over bar indices `[s, e)` in `O(log n)`.
    pub fn price_range(&self, s: usize, e: usize) -> Option<(f64, f64)> {
        self.pyramid.query(s, e).map(|b| (b.low, b.high))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CandleStyle {
    pub up: Rgba,
    pub down: Rgba,
    /// Body width as a fraction of the bar slot (0..1).
    pub body: f32,
}

impl Default for CandleStyle {
    fn default() -> Self {
        Self {
            up: Rgba::rgb(38, 166, 154),
            down: Rgba::rgb(239, 83, 80),
            body: 0.7,
        }
    }
}

/// Lower the visible candles of LOD level `k` into rect instances.
/// Coordinates: x = bar index - `origin.0`, y = price - `origin.1`.
/// Returns the number of candles emitted.
pub fn candles(
    series: &OhlcSeries,
    level: usize,
    range: (usize, usize),
    origin: (f64, f64),
    style: &CandleStyle,
    out: &mut Vec<Rect>,
) -> usize {
    let (first, items) = series.pyramid.visible(level, range.0, range.1);
    let slot = (1usize << level.min(series.pyramid.max_level())) as f64;
    out.reserve(items.len() * 2);
    for (i, b) in items.iter().enumerate() {
        let x0 = first as f64 + i as f64 * slot - origin.0;
        let cx = (x0 + slot * 0.5) as f32;
        let half = (slot * style.body as f64 * 0.5) as f32;
        let color = if b.close >= b.open {
            style.up
        } else {
            style.down
        };
        let (o, c) = ((b.open - origin.1) as f32, (b.close - origin.1) as f32);
        let (lo, hi) = ((b.low - origin.1) as f32, (b.high - origin.1) as f32);
        out.push(Rect::new([cx, lo], [cx, hi], color).with_min_px(1.0, 1.0));
        out.push(
            Rect::new([cx - half, o.min(c)], [cx + half, o.max(c)], color).with_min_px(1.0, 1.0),
        );
    }
    items.len()
}

/// Average volume per base bar of an LOD item: `volume / bars covered`.
/// Plotting this (instead of the summed volume) keeps bar heights stable when
/// the zoom crosses an LOD level, so the volume pane never "bounces".
pub fn avg_volume(series: &OhlcSeries, level: usize, first: usize, i: usize, b: &Bar) -> f64 {
    let slot = 1usize << level.min(series.pyramid.max_level());
    let start = first + i * slot;
    let count = slot.min(series.len().saturating_sub(start)).max(1);
    b.volume / count as f64
}

/// Lower visible volume bars of LOD level `k` into rects coloured by candle
/// direction. Heights are [`avg_volume`] (per base bar), so they don't change
/// with the LOD level. Returns the largest visible height, for fitting the pane.
pub fn volume(
    series: &OhlcSeries,
    level: usize,
    range: (usize, usize),
    origin_x: f64,
    style: &CandleStyle,
    alpha: u8,
    out: &mut Vec<Rect>,
) -> f64 {
    let (first, items) = series.pyramid.visible(level, range.0, range.1);
    let slot = (1usize << level.min(series.pyramid.max_level())) as f64;
    let mut max = 0.0f64;
    for (i, b) in items.iter().enumerate() {
        let x0 = first as f64 + i as f64 * slot - origin_x;
        let cx = (x0 + slot * 0.5) as f32;
        let half = (slot * style.body as f64 * 0.5) as f32;
        let c = if b.close >= b.open {
            style.up
        } else {
            style.down
        };
        let color = Rgba((c.0 & !0xff) | alpha as u32);
        let v = avg_volume(series, level, first, i, b);
        out.push(Rect::new([cx - half, 0.0], [cx + half, v as f32], color).with_min_px(1.0, 0.0));
        max = max.max(v);
    }
    max
}

/// Synthetic order-book liquidity for bars `[start, end)`: one heatmap column
/// per bar, `rows` price buckets. Liquidity clusters near price, with
/// persistent "walls" that appear, linger and get eaten when price trades
/// through them. For demos and benchmarks.
pub fn synthetic_book(bars: &[Bar], start: usize, rows: usize, seed: u64) -> Heatmap {
    let span = &bars[start.min(bars.len())..];
    let lo = span.iter().map(|b| b.low).fold(f64::INFINITY, f64::min);
    let hi = span
        .iter()
        .map(|b| b.high)
        .fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if lo.is_finite() && hi > lo {
        (lo, hi)
    } else {
        (0.0, 1.0)
    };
    let pad = (hi - lo) * 0.1;
    let (y0, dy) = (lo - pad, (hi - lo + 2.0 * pad) / rows as f64);
    let mut h = Heatmap::new(rows, start.min(bars.len()), y0, dy);
    let mut state = seed.max(1);
    let mut rnd = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f32 / (1u64 << 53) as f32
    };
    // (row, size, remaining life)
    let mut walls: Vec<(usize, f32, u32)> = Vec::new();
    let mut col = vec![0.0f32; rows];
    for b in span {
        let pr = ((b.close - y0) / dy) as f32;
        if rnd() < 0.04 {
            let off = (rnd() - 0.5) * rows as f32 * 0.5;
            let r = (pr + off).clamp(0.0, rows as f32 - 1.0) as usize;
            walls.push((r, 2.0 + rnd() * 5.0, 200 + (rnd() * 3000.0) as u32));
        }
        let (bl, bh) = (((b.low - y0) / dy) as usize, ((b.high - y0) / dy) as usize);
        walls.retain_mut(|w| {
            w.2 = w.2.saturating_sub(1);
            w.2 > 0 && !(w.0 >= bl && w.0 <= bh && rnd() < 0.5)
        });
        for (r, v) in col.iter_mut().enumerate() {
            let d = (r as f32 - pr).abs();
            *v = (2.0 * (-d / (rows as f32 * 0.06)).exp() + 0.3 * rnd()) * (0.6 + 0.4 * rnd());
        }
        for w in &walls {
            col[w.0] += w.1;
        }
        h.push_column(&col);
    }
    h
}

/// Simple moving average over closes. `NaN` until `period` bars are seen,
/// and `NaN` while the window contains a non-finite close; a bad tick does
/// not poison later values.
pub fn sma(bars: &[Bar], period: usize) -> Vec<f64> {
    let mut out = vec![f64::NAN; bars.len()];
    if period == 0 {
        return out;
    }
    let (mut sum, mut bad) = (0.0, 0usize);
    for i in 0..bars.len() {
        let v = bars[i].close;
        if v.is_finite() {
            sum += v
        } else {
            bad += 1
        }
        if i >= period {
            let old = bars[i - period].close;
            if old.is_finite() {
                sum -= old
            } else {
                bad -= 1
            }
        }
        if i + 1 >= period && bad == 0 {
            out[i] = sum / period as f64;
        }
    }
    out
}

/// Exponential moving average over closes, seeded with the first close.
pub fn ema(bars: &[Bar], period: usize) -> Vec<f64> {
    let k = 2.0 / (period as f64 + 1.0);
    let mut prev = f64::NAN;
    bars.iter()
        .map(|b| {
            prev = if prev.is_nan() {
                b.close
            } else {
                b.close * k + prev * (1.0 - k)
            };
            prev
        })
        .collect()
}

/// An indicator line (or any per-bar series) with a min/max pyramid, so
/// zoomed-out rendering keeps every spike (M4-style decimation).
#[derive(Clone, Debug, Default)]
pub struct LineSeries {
    pub pyramid: Pyramid<MinMax>,
}

impl LineSeries {
    pub fn new(values: &[f64]) -> Self {
        Self {
            pyramid: Pyramid::build(values.iter().map(|v| MinMax::point(*v)).collect()),
        }
    }
    pub fn push(&mut self, v: f64) {
        self.pyramid.push(MinMax::point(v))
    }
}

/// Lower the visible part of a line at LOD `level` into segments. At level 0
/// it is the exact polyline; above, each slot contributes its min and max, so
/// no extreme is ever dropped. Non-finite values break the line.
pub fn line(
    series: &LineSeries,
    level: usize,
    range: (usize, usize),
    origin: (f64, f64),
    color: Rgba,
    width_px: f32,
    out: &mut Vec<Segment>,
) {
    let level = level.min(series.pyramid.max_level());
    let (first, items) = series.pyramid.visible(level, range.0, range.1);
    let slot = (1usize << level) as f64;
    let mut run: Vec<[f32; 2]> = Vec::with_capacity(items.len() * 2);
    let flush = |run: &mut Vec<[f32; 2]>, out: &mut Vec<Segment>| {
        polyline(run, color, width_px, out);
        run.clear();
    };
    for (i, mm) in items.iter().enumerate() {
        if !mm.min.is_finite() || !mm.max.is_finite() {
            flush(&mut run, out);
            continue;
        }
        let x = (first as f64 + i as f64 * slot + slot * 0.5 - origin.0) as f32;
        run.push([x, (mm.min - origin.1) as f32]);
        if mm.max != mm.min {
            run.push([x, (mm.max - origin.1) as f32]);
        }
    }
    flush(&mut run, out);
}

/// Deterministic random-walk bars for demos and benchmarks (1-minute bars).
pub fn random_walk(n: usize, seed: u64) -> Vec<Bar> {
    let mut state = seed.max(1);
    let mut rnd = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    };
    // log-space walk with weak mean reversion: realistic, never drifts to 0
    let mut logp = 100f64.ln();
    let mut price = 100.0f64;
    let t0 = 1_600_000_000_000i64;
    (0..n)
        .map(|i| {
            let open = price;
            logp += (rnd() - 0.5) * 0.004 - (logp - 100f64.ln()) * 2e-5;
            let close = logp.exp();
            let high = open.max(close) + rnd() * 0.006 * price;
            let low = (open.min(close) - rnd() * 0.006 * price).max(0.001);
            price = close;
            Bar::new(
                t0 + i as i64 * 60_000,
                open,
                high,
                low,
                close,
                100.0 + rnd() * 1000.0,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_range_matches_brute_force() {
        let bars = random_walk(5000, 7);
        let s = OhlcSeries::new(bars.clone());
        for (a, b) in [(0, 5000), (3, 4097), (1000, 1001), (17, 2999)] {
            let lo = bars[a..b]
                .iter()
                .map(|x| x.low)
                .fold(f64::INFINITY, f64::min);
            let hi = bars[a..b]
                .iter()
                .map(|x| x.high)
                .fold(f64::NEG_INFINITY, f64::max);
            assert_eq!(s.price_range(a, b), Some((lo, hi)));
        }
    }

    #[test]
    fn aggregated_candle_keeps_open_close() {
        let bars = random_walk(64, 1);
        let s = OhlcSeries::new(bars.clone());
        let top = s.pyramid.level(6)[0];
        assert_eq!(top.open, bars[0].open);
        assert_eq!(top.close, bars[63].close);
    }

    #[test]
    fn candles_emit_two_rects_each() {
        let s = OhlcSeries::new(random_walk(1000, 2));
        let mut out = Vec::new();
        let n = candles(
            &s,
            2,
            (100, 200),
            (100.0, 100.0),
            &CandleStyle::default(),
            &mut out,
        );
        assert_eq!(out.len(), n * 2);
        assert!((25..=26).contains(&n));
    }

    #[test]
    fn line_lod_keeps_spike() {
        let mut v = vec![1.0; 1024];
        v[517] = 50.0;
        let ls = LineSeries::new(&v);
        let mut out = Vec::new();
        line(
            &ls,
            6,
            (0, 1024),
            (0.0, 0.0),
            Rgba::rgb(1, 2, 3),
            1.0,
            &mut out,
        );
        assert!(out.iter().any(|s| s.a[1] == 50.0 || s.b[1] == 50.0));
    }

    #[test]
    fn sma_recovers_after_nan() {
        let mut bars: Vec<Bar> = (1..=6)
            .map(|i| Bar {
                close: i as f64,
                ..Default::default()
            })
            .collect();
        bars[1].close = f64::NAN;
        let s = sma(&bars, 2);
        assert!(s[1].is_nan() && s[2].is_nan());
        assert_eq!(&s[3..], &[3.5, 4.5, 5.5]);
    }

    #[test]
    fn volume_and_book() {
        let bars = random_walk(5000, 3);
        let s = OhlcSeries::new(bars.clone());
        let mut out = Vec::new();
        let max = volume(
            &s,
            0,
            (100, 200),
            100.0,
            &CandleStyle::default(),
            140,
            &mut out,
        );
        assert_eq!(out.len(), 100);
        assert!(max > 0.0 && out.iter().all(|r| r.max[1] as f64 <= max + 1e-3));
        // per-bar heights are level-independent: level 3 average <= level 0 max
        let mut coarse = Vec::new();
        let cmax = volume(
            &s,
            3,
            (96, 200),
            96.0,
            &CandleStyle::default(),
            140,
            &mut coarse,
        );
        assert!(
            cmax <= 1100.0 && cmax > 100.0,
            "avg volume per bar, got {cmax}"
        );
        let h = synthetic_book(&bars, 1000, 64, 9);
        assert_eq!(h.cols(), 4000);
        assert!(h.max_value() > 0.0);
    }

    #[test]
    fn sma_values() {
        let bars: Vec<Bar> = (1..=5)
            .map(|i| Bar {
                close: i as f64,
                ..Default::default()
            })
            .collect();
        let s = sma(&bars, 2);
        assert!(s[0].is_nan());
        assert_eq!(&s[1..], &[1.5, 2.5, 3.5, 4.5]);
    }
}
