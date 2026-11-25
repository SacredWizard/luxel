//! luxel demo: millions of candles + order-book heatmap + volume, GPU-rendered
//! inside egui via the `luxel-egui` widget.
//!
//! Native: `cargo run -p luxel-demo --release`
//! Web:    `examples/luxel-demo/build-web.sh`, then serve `examples/luxel-demo/web`.
//!
//! Options (env vars natively, URL query on the web, e.g. `?bars=10000000&bench=1`):
//! * `bars`: dataset size (default 1,000,000)
//! * `bench`: run a scripted zoom sweep and print frame-time stats
//! * `view=all`: start fully zoomed out
//! * `shot=path.ppm` (native only): save a screenshot after a few frames and exit

use eframe::egui;
use luxel_core::{Heatmap, Rgba};
use luxel_egui::{Chart, ChartState, ChartStyle};
use luxel_finance::{LineSeries, OhlcSeries, ema, random_walk, sma, synthetic_book};

/// Bars of synthetic order-book history (one heatmap column per bar).
const BOOK_BARS: usize = 20_000;
const BOOK_ROWS: usize = 160;

struct App {
    series: OhlcSeries,
    sma50: LineSeries,
    ema200: LineSeries,
    book: Heatmap,
    chart: ChartState,
    show_ma: bool,
    show_volume: bool,
    show_book: bool,
    frames: u64,
    ui_ms: f64,
    bench: Option<Bench>,
    load_ms: f64,
}

/// Scripted zoom sweep: from 300 bars to the whole series and back, panning
/// along the way; records frame intervals and per-frame CPU time.
struct Bench {
    frame: usize,
    total: usize,
    last: f64,
    intervals: Vec<f64>,
    cpu: Vec<f64>,
    /// Price-axis range each frame, to measure how smoothly it moves.
    axis: Vec<(f64, f64)>,
    done: bool,
}

impl Bench {
    fn new() -> Self {
        Self {
            frame: 0,
            total: 600,
            last: 0.0,
            intervals: Vec::new(),
            axis: Vec::new(),
            cpu: Vec::new(),
            done: false,
        }
    }
}

fn option(name: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var(format!("LUXEL_{}", name.to_uppercase())).ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let search = web_sys::window()?.location().search().ok()?;
        search.trim_start_matches('?').split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, "1"));
            (k == name).then(|| v.to_string())
        })
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        luxel_egui::install(
            cc.wgpu_render_state
                .as_ref()
                .expect("luxel-demo needs the wgpu renderer"),
        );
        let n = option("bars")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_000_000);
        let mut app = Self {
            series: OhlcSeries::default(),
            sma50: LineSeries::default(),
            ema200: LineSeries::default(),
            book: Heatmap::new(1, 0, 0.0, 1.0),
            chart: ChartState::default(),
            show_ma: true,
            show_volume: true,
            show_book: true,
            frames: 0,
            ui_ms: 0.0,
            bench: option("bench").map(|_| Bench::new()),
            load_ms: 0.0,
        };
        app.load(n);
        if option("view").as_deref() == Some("all") {
            app.chart.show_all(n);
        }
        app
    }

    fn load(&mut self, n: usize) {
        let bars = random_walk(n, 42);
        self.sma50 = LineSeries::new(&sma(&bars, 50));
        self.ema200 = LineSeries::new(&ema(&bars, 200));
        self.book = synthetic_book(&bars, n.saturating_sub(BOOK_BARS), BOOK_ROWS, 7);
        let t = now_ms();
        self.series = OhlcSeries::new(bars);
        self.load_ms = now_ms() - t; // OHLC pyramid build (comparable to setData)
        self.chart.show_last(n, 300);
        self.chart.data_changed();
    }

    fn drive_bench(&mut self, ctx: &egui::Context) {
        let n = self.series.len();
        let Some(b) = &mut self.bench else { return };
        if b.done {
            return;
        }
        let t = now_ms();
        if b.frame > 0 {
            b.intervals.push(t - b.last);
            b.cpu.push(self.ui_ms);
            b.axis.push(self.chart.cam.y);
        }
        b.last = t;
        // 0..1..0 sweep, log-spaced visible width from 300 bars to all bars
        let p = b.frame as f64 / b.total as f64;
        let s = 1.0 - (2.0 * p - 1.0).abs();
        let w = 300f64 * (n as f64 / 300.0).max(1.0).powf(s);
        let end = n as f64 - (n as f64 - w).max(0.0) * 0.3 * s;
        self.chart.cam.x = (end - w, end);
        b.frame += 1;
        ctx.request_repaint();
        if b.frame > b.total {
            b.done = true;
            let stats = |v: &mut Vec<f64>| {
                v.sort_by(|a, b| a.total_cmp(b));
                let q = |f: f64| v[((v.len() - 1) as f64 * f) as usize];
                (
                    v.iter().sum::<f64>() / v.len() as f64,
                    q(0.5),
                    q(0.95),
                    q(0.99),
                    *v.last().unwrap(),
                )
            };
            // axis smoothness: largest single-frame move and p99 per-frame
            // acceleration of either axis end, in % of the axis height
            let ax = &b.axis;
            let mut step = 0.0f64;
            let mut acc: Vec<f64> = Vec::new();
            for i in 1..ax.len() {
                let h = (ax[i].1 - ax[i].0).max(1e-12);
                let d = ((ax[i].0 - ax[i - 1].0).abs()).max((ax[i].1 - ax[i - 1].1).abs());
                step = step.max(d / h * 100.0);
                if i >= 2 {
                    let a0 = ax[i].0 - 2.0 * ax[i - 1].0 + ax[i - 2].0;
                    let a1 = ax[i].1 - 2.0 * ax[i - 1].1 + ax[i - 2].1;
                    acc.push(a0.abs().max(a1.abs()) / h * 100.0);
                }
            }
            acc.sort_by(|a, b| a.total_cmp(b));
            let acc99 = acc
                .get((acc.len().max(1) - 1) * 99 / 100)
                .copied()
                .unwrap_or(0.0);
            let (im, i50, i95, i99, imax) = stats(&mut b.intervals);
            let (cm, c50, c95, c99, cmax) = stats(&mut b.cpu);
            let json = format!(
                "{{\"engine\":\"luxel\",\"bars\":{n},\"load_ms\":{:.0},\"frames\":{},\"fps\":{:.1},\"interval_ms\":{{\"mean\":{im:.2},\"p50\":{i50:.2},\"p95\":{i95:.2},\"p99\":{i99:.2},\"max\":{imax:.2}}},\"cpu_ms\":{{\"mean\":{cm:.3},\"p50\":{c50:.3},\"p95\":{c95:.3},\"p99\":{c99:.3},\"max\":{cmax:.3}}},\"rebuilds\":{},\"axis_max_step_pct\":{step:.2},\"axis_accel_p99_pct\":{acc99:.3}}}",
                self.load_ms,
                b.total,
                1000.0 / im,
                self.chart.stats.rebuilds
            );
            log_line(&format!("LUXEL_BENCH {json}"));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let t0 = now_ms();
        self.drive_bench(&ctx);
        egui::Panel::top("bar").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.strong("luxel");
                for (label, n) in [("100K", 100_000), ("1M", 1_000_000), ("10M", 10_000_000)] {
                    if ui
                        .selectable_label(self.series.len() == n, format!("{label} bars"))
                        .clicked()
                    {
                        self.load(n);
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.show_ma, "SMA50 / EMA200");
                ui.checkbox(&mut self.show_volume, "Volume");
                ui.checkbox(&mut self.show_book, "Order book (last 20K bars)");
                ui.separator();
                let s = self.chart.stats;
                ui.label(format!(
                    "LOD {} | {} instances | rebuild {:.2} ms | ui {:.2} ms | {:.0} fps",
                    s.lod_level,
                    s.instances,
                    s.build_ms,
                    self.ui_ms,
                    1.0 / ctx.input(|i| i.stable_dt).max(1e-6)
                ));
            });
        });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(18, 20, 26)))
            .show(root, |ui| {
                let mut style = ChartStyle::default();
                if let Some(t) = option("smooth").and_then(|v| v.parse().ok()) {
                    style.y_smooth_secs = t; // e.g. smooth=0 to compare with snapping
                }
                let mut chart = Chart::new("main", &self.series)
                    .volume(self.show_volume)
                    .style(style);
                if self.show_book {
                    chart = chart.heatmap(&self.book);
                }
                if self.show_ma {
                    chart = chart.line(&self.sma50, Rgba::rgb(255, 193, 7), 1.5).line(
                        &self.ema200,
                        Rgba::rgb(171, 71, 188),
                        1.5,
                    );
                }
                chart.show(ui, &mut self.chart);
            });
        self.ui_ms = now_ms() - t0;
        self.frames += 1;
        self.debug_screenshot(&ctx);
    }
}

impl App {
    /// `LUXEL_SHOT=out.ppm` saves a screenshot after a few frames and exits.
    #[cfg(not(target_arch = "wasm32"))]
    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = option("shot") else { return };
        let wait = if self.bench.is_some() { 620 } else { 30 };
        if self.frames == wait {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
            for p in &img.pixels {
                ppm.extend_from_slice(&[p.r(), p.g(), p.b()]);
            }
            std::fs::write(&path, ppm).unwrap();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    #[cfg(target_arch = "wasm32")]
    fn debug_screenshot(&mut self, _ctx: &egui::Context) {}
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.0
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn log_line(s: &str) {
    eprintln!("{s}");
}

#[cfg(target_arch = "wasm32")]
fn log_line(s: &str) {
    web_sys::console::log_1(&s.into());
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        doc.set_title(s);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 800.0])
            .with_title("luxel demo"),
        ..Default::default()
    };
    eframe::run_native(
        "luxel demo",
        opts,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    std::panic::set_hook(Box::new(|p| {
        web_sys::console::error_1(&format!("luxel panic: {p}").into())
    }));
    wasm_bindgen_futures::spawn_local(async {
        let window = web_sys::window().expect("window");
        let doc = window.document().expect("document");
        // eframe drops the initial font-texture upload if its first frame runs
        // while the page is hidden, then panics on the next frame. Wait until
        // the page is visible (re-checked after every visibility change) and
        // start in the same task, so there is no gap for it to hide again.
        while doc.hidden() {
            let p = js_sys::Promise::new(&mut |resolve, _| {
                let _ = doc.add_event_listener_with_callback("visibilitychange", &resolve);
            });
            let _ = wasm_bindgen_futures::JsFuture::from(p).await;
        }
        let canvas = doc
            .get_element_by_id("luxel")
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("canvas #luxel");
        eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(App::new(cc)))),
            )
            .await
            .expect("start");
    });
}
