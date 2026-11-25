# photic

**GPU-native data visualisation for Rust: native and in the browser.**

photic renders millions of data points at interactive frame rates by doing two
things differently from SVG/Canvas2D libraries:

1. **Level-of-detail pyramids.** Every series carries a pyramid of
   pre-aggregated levels (true min/max, OHLC, sums). The per-frame work is
   `O(log n + screen pixels)`, so 50 million bars cost the same per frame as
   100 thousand.
2. **Instanced GPU primitives.** Everything (candles, bars, heatmap cells,
   scatter points, lines) is lowered into three instance types (`Rect`,
   `Segment`, `Point`) and drawn with **one draw call per type** through
   [wgpu](https://wgpu.rs). Instance data stays on the GPU; panning only
   rewrites a 32-byte uniform.

It runs on Metal, Vulkan, DX12, **WebGPU and WebGL2** from the same code.

## Crates

| crate | what | deps |
|---|---|---|
| [`photic-core`](crates/photic-core) | `Pyramid<T: Aggregate>` (generic LOD tree), `Heatmap` (column LOD by max), d3-style scales (linear, log, time, band), nice ticks, `Camera2D`, GPU primitive types | `bytemuck` |
| [`photic-gpu`](crates/photic-gpu) | wgpu renderer: `Renderer` (pipelines) + `Layer` (instance buffers + view) | `wgpu` |
| [`photic-finance`](crates/photic-finance) | OHLCV `Bar`, `OhlcSeries`, candle lowering, `LineSeries` (min/max LOD lines), SMA/EMA, streaming `push`/`update_last` | core |
| [`photic-egui`](crates/photic-egui) | Ready-made egui chart widget: candles, indicator lines, order-book heatmap, volume pane, eased price axis, axes, crosshair | `egui`, `egui-wgpu` |
| [`photic-demo`](examples/photic-demo) | egui app (native + browser): 100K / 1M / 10M bars, heatmap, volume, scripted benchmark mode | eframe |

`photic-core` has no GPU or GUI dependency. `photic-gpu` depends only on
`wgpu`, so it plugs into **egui, iced, bevy, winit or headless** rendering.

## Try it

```bash
cargo run -p photic-demo --release
```

```bash
cargo run -p photic-gpu --example offscreen -- out.ppm
```

```bash
cargo run -p photic-finance --release --example bench
```

Browser (WebGPU; add wgpu's `webgl` feature for the WebGL2 fallback, which
the demo gets through egui-wgpu):

```bash
cargo install wasm-bindgen-cli --version <version of wasm-bindgen in Cargo.lock>
```

```bash
./examples/photic-demo/build-web.sh
```

```bash
python3 -m http.server -d examples/photic-demo/web 8080
```

## Numbers (Apple Silicon, release build)

CPU time per frame for the visible range: price-range query plus candle
instance generation, measured by `examples/bench.rs` at a 2000 px plot width.

| bars | pyramid build | range query | frame, whole series visible | frame, last 500 bars |
|---:|---:|---:|---:|---:|
| 100K | 0.6 ms | 0.03 µs | 2.3 µs | 1.4 µs |
| 1M | 5.7 ms | 0.04 µs | 2.2 µs | 1.1 µs |
| 10M | 70 ms | 0.03 µs | 1.2 µs | 0.9 µs |
| 50M | 790 ms | 0.03 µs | 1.7 µs | 1.9 µs |

In the demo, 10M bars fully zoomed out rebuild their instances in about 1.2 ms
and the app runs at the display refresh rate (about 120 fps on a 120 Hz
screen).

### In the browser: photic vs TradingView lightweight-charts

The same scripted scenario runs in both: 600 frames sweeping the visible window
from 300 bars to the **whole series** and back, panning along the way. Same
browser (headless Chrome, which paces frames at 60 Hz), same 1400×800
viewport, same machine (Apple Silicon), same random-walk data shape.

| bars | library | load | mean fps | p50 frame | p95 frame | p99 frame |
|---:|---|---:|---:|---:|---:|---:|
| 100K | photic | 1 ms | **60.1** | 16.7 ms | 17.6 ms | 17.9 ms |
| 100K | lightweight-charts 4.2.0 | 39 ms | 51.0 | 16.7 ms | 49.9 ms | 50.1 ms |
| 1M | photic | 8 ms | **60.0** | 16.7 ms | 16.9 ms | 17.0 ms |
| 1M | lightweight-charts 4.2.0 | 454 ms | 11.6 | 16.7 ms | 433 ms | 617 ms |
| 10M | photic | 119 ms | **60.0** | 16.7 ms | 17.5 ms | 17.7 ms |
| 10M | lightweight-charts 4.2.0 | did not finish within 4 min | | | | |

**Visible Chrome window, 120 Hz display** (same scenario, same machine). The
window was briefly covered during some runs, which caused multi-second
pauses that make mean fps meaningless, so this table uses frame-time
percentiles:

| bars | library | p50 frame | p95 frame | p99 frame |
|---:|---|---:|---:|---:|
| 100K | photic | 8.3 ms | **9.1 ms** | **9.4 ms** |
| 100K | lightweight-charts 4.2.0 | 8.4 ms | 41.1 ms | 66.8 ms |
| 1M | photic | 8.3 ms | **9.0 ms** | **9.3 ms** |
| 1M | lightweight-charts 4.2.0 | 9.2 ms | 516 ms | 717 ms |
| 10M | photic | 8.3 ms | **9.0 ms** | **9.3 ms** |
| 10M | lightweight-charts 4.2.0 | did not finish within 3 min | | |

"Load" is building photic's pyramid vs `series.setData()`. photic's run also
draws an order-book heatmap, a volume pane and two indicators;
lightweight-charts draws candles only. lightweight-charts' default
`minBarSpacing` stops you from zooming out this far, so the bench lowers it to
make the scenario possible. Treat this as one machine and one scenario, not a
general verdict. Reproduce it:

```bash
./examples/photic-demo/build-web.sh
```

```bash
python3 -m http.server -d examples/photic-demo/web 8080
```

**Axis smoothness.** The same 1M-bar sweep also records the price axis every
frame. Snapping the axis to the visible range (`smooth=0`, the old
behaviour) moves it by up to 222% of its height in one frame (p99 per-frame
acceleration 171%). The spring (`ChartStyle::y_smooth_secs = 0.12`) cuts that to
34% (p99 10%), and that worst case only happens because the sweep zooms ~4% per frame.

Then open `http://localhost:8080/?bars=1000000&bench=1` and
`http://localhost:8080/bench-lwc.html?bars=1000000`. Each prints a
`*_BENCH {json}` line to the console and the page title. Use a visible tab: browsers throttle hidden ones.

## Using it

### With egui (easiest)

```rust
use photic_egui::{Chart, ChartState};

// once, in your eframe::App constructor (wgpu renderer):
photic_egui::install(cc.wgpu_render_state.as_ref().unwrap());

// every frame:
Chart::new("btc", &series)                        // OhlcSeries
    .line(&sma50, Rgba::rgb(255, 193, 7), 1.5)    // LineSeries
    .heatmap(&order_book)                         // Heatmap
    .volume(true)
    .show(ui, &mut self.chart_state);             // ChartState
```

The price and volume axes follow the visible range with a critically damped
spring instead of snapping, so zooming feels continuous
(`ChartStyle::y_smooth_secs`, 0 to snap). Volume is plotted per bar, so its
scale doesn't jump when the zoom crosses an LOD level.

### With any wgpu host

```rust
use photic_core::{lod_level_for, Camera2D};
use photic_finance::{candles, CandleStyle, OhlcSeries};
use photic_gpu::{Layer, Renderer};

let series = OhlcSeries::new(bars);                 // builds the LOD pyramid
let renderer = Renderer::new(&device, format, 1);   // once per device
let mut layer = Layer::new(&device, &renderer);     // one per plot pane

// when the visible window or zoom level changes:
let level = lod_level_for(cam.x_per_px(), 0.25, series.pyramid.max_level());
let origin = (first_visible as f64, base_price);    // keeps f32 precise
let mut rects = Vec::new();
candles(&series, level, (first_visible, last_visible), origin, &CandleStyle::default(), &mut rects);
layer.set_rects(&device, &queue, &rects);

// every frame (32 bytes):
layer.set_view(&queue, cam.gpu_transform(origin), [width_px, height_px]);
renderer.draw(&mut render_pass, &layer);
```

## Design notes

* **Precision:** data is uploaded as `f32(value - origin)` and the camera
  is kept in `f64`, so Unix-millisecond timestamps, prices and lat/lon
  survive the trip to the GPU.
* **No lost extremes:** pyramid aggregates keep the true high/low (min/max)
  for candles and for lines (`LineSeries`, M4-style), so a spike is visible
  at every zoom level, unlike averaging or stride sampling.
* **Crisp at any zoom:** rects are snapped to the pixel grid in the vertex
  shader and have a minimum on-screen size, so 1 px wicks and dojis never
  vanish or shimmer while panning.
* **Bars by index, not time:** session gaps (nights, weekends) never show
  as empty space; the time axis is labelled from the bar's timestamp.
* **WebGL2-safe:** only vertex/instance buffers and one uniform; no storage
  buffers or compute, so the WebGL2 fallback works.

## Roadmap

photic is meant to be a general GPU visualisation engine; finance is the
first layer on top of it.

**Engine (photic-core / photic-gpu)**
- [ ] GPU text (SDF glyph atlas) so axes/labels don't need a host UI
- [x] Line LOD via min/max envelope (M4) for indicator and time-series lines
- [ ] Proper polyline joins (miter/bevel) for translucent lines
- [ ] Area/band fills, gradients, dashed lines, line joins
- [x] Heatmap layer with column LOD (order-book liquidity, spectrograms, matrices)
- [ ] Heatmap as a GPU texture (fewer instances for very dense grids)
- [ ] Partial buffer updates for streaming (append-only `write_buffer` ranges)
- [ ] Hit testing / picking (GPU id buffer) for tooltips and selection
- [ ] MSAA option and HiDPI-aware line widths
- [ ] Calendar-accurate month/year ticks and time zones

**General charts (d3-style, GPU-backed)**
- [ ] `photic-charts`: line, area, bar, scatter, histogram, heatmap, candlestick
      as declarative specs over the same primitives
- [ ] Large scatter plots (10M+ points) with density-based LOD
- [ ] Graph/network layout and rendering, maps/projections (longer term)
- [ ] JS/TypeScript bindings via wasm-bindgen (`npm i photic`) so web apps can use it

**Finance (photic-finance)**
- [x] Volume pane
- [ ] Multiple synced panes, price-axis drag to scale
- [ ] More indicators (RSI, MACD, Bollinger, VWAP), incremental on `push`
- [ ] Drawing tools (trendline, fib, rectangle) anchored in data space
- [ ] Footprint / volume profile / order-flow from tick data
- [ ] Renko, range and volume bars

**Integrations**
- [x] `photic-egui` widget crate
- [ ] iced and bevy examples
- [x] Head-to-head benchmark vs lightweight-charts
- [x] Benchmark in a visible browser at 120 Hz
- [ ] Benchmarks vs ECharts / D3

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
