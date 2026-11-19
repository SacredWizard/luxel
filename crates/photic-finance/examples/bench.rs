//! CPU-side benchmark of the per-frame work: LOD build, visible-range price
//! query and candle instance generation. GPU time is separate (one draw
//! call per primitive kind).
//! `cargo run -p photic-finance --release --example bench`
use std::hint::black_box;
use std::time::Instant;

use photic_core::lod_level_for;
use photic_finance::{candles, random_walk, CandleStyle, OhlcSeries};

fn time<T>(iters: u32, mut f: impl FnMut() -> T) -> f64 {
    let t = Instant::now();
    for _ in 0..iters {
        black_box(f());
    }
    t.elapsed().as_secs_f64() * 1e6 / iters as f64
}

fn main() {
    let width_px = 2000.0;
    println!("{:>11} | {:>9} | {:>12} | {:>16} | {:>16}", "bars", "build ms", "query µs", "frame µs (all)", "frame µs (500)");
    for n in [100_000usize, 1_000_000, 10_000_000, 50_000_000] {
        let bars = random_walk(n, 1);
        let t = Instant::now();
        let s = OhlcSeries::new(bars);
        let build_ms = t.elapsed().as_secs_f64() * 1e3;
        let q = time(100_000, || s.price_range(n / 7, n - n / 5));
        let style = CandleStyle::default();
        let mut out = Vec::with_capacity(8192);
        // whole dataset on screen (worst case for naive renderers)
        let lvl_all = lod_level_for(n as f64 / width_px, 0.25, s.pyramid.max_level());
        let all = time(200, || {
            out.clear();
            s.price_range(0, n);
            candles(&s, lvl_all, (0, n), (0.0, 100.0), &style, &mut out)
        });
        // typical: last 500 bars
        let lvl = lod_level_for(500.0 / width_px, 0.25, s.pyramid.max_level());
        let recent = time(2000, || {
            out.clear();
            s.price_range(n - 500, n);
            candles(&s, lvl, (n - 500, n), ((n - 500) as f64, 100.0), &style, &mut out)
        });
        println!("{n:>11} | {build_ms:>9.1} | {q:>12.3} | {all:>16.1} | {recent:>16.1}");
    }
}
