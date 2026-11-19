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
| [`photic-core`](crates/photic-core) | `Pyramid<T: Aggregate>` (generic LOD tree), d3-style scales (linear, log, time, band), nice ticks, `Camera2D`, GPU primitive types | `bytemuck` |
| [`photic-gpu`](crates/photic-gpu) | wgpu renderer: `Renderer` (pipelines) + `Layer` (instance buffers + view) | `wgpu` |
| [`photic-finance`](crates/photic-finance) | OHLCV `Bar`, `OhlcSeries`, candle lowering, `LineSeries` (min/max LOD lines), SMA/EMA, streaming `push`/`update_last` | core |
| [`photic-demo`](examples/photic-demo) | egui app: drag to pan, wheel to zoom, crosshair, 100K / 1M / 10M bars | eframe |

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

**Not yet measured:** a head-to-head against TradingView's
`lightweight-charts`, ECharts or D3 on the same machine and dataset. That
benchmark is the next milestone; until then, treat the numbers above as
photic's own and not as a comparison.

## Using it

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
- [ ] Heatmap texture layer (order-book liquidity, spectrograms, matrices)
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
- [ ] Volume pane, multiple synced panes
- [ ] More indicators (RSI, MACD, Bollinger, VWAP), incremental on `push`
- [ ] Drawing tools (trendline, fib, rectangle) anchored in data space
- [ ] Footprint / volume profile / order-flow from tick data
- [ ] Renko, range and volume bars

**Integrations**
- [ ] `photic-egui` widget crate (wraps the demo's callback into `ui.add(Chart::new(...))`)
- [ ] iced and bevy examples
- [ ] Head-to-head benchmark suite vs lightweight-charts / ECharts / D3

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
