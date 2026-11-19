//! photic demo: millions of candles, GPU-rendered inside egui.
//! Native: `cargo run -p photic-demo --release`
//! Web:    see README (trunk / wasm-bindgen).

use std::sync::Arc;

use eframe::egui;
use egui_wgpu::wgpu;
use photic_core::{format_utc, lod_level_for, nice_ticks, Camera2D, Rect, Rgba, Segment};
use photic_finance::{candles, ema, line, random_walk, sma, CandleStyle, LineSeries, OhlcSeries};
use photic_gpu::{Layer, Renderer};

/// GPU objects living in egui-wgpu's callback resources.
struct Gpu {
    renderer: Renderer,
    layer: Layer,
}

/// Instance data built on the CPU; uploaded only when it changes.
#[derive(Default)]
struct Instances {
    rects: Vec<Rect>,
    segs: Vec<Segment>,
}

struct Frame {
    upload: Option<Arc<Instances>>,
    transform: photic_core::GpuTransform,
    rect: egui::Rect,
}

impl egui_wgpu::CallbackTrait for Frame {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _enc: &mut wgpu::CommandEncoder,
        res: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let gpu: &mut Gpu = res.get_mut().unwrap();
        if let Some(inst) = &self.upload {
            gpu.layer.set_rects(device, queue, &inst.rects);
            gpu.layer.set_segments(device, queue, &inst.segs);
        }
        let ppp = screen.pixels_per_point;
        gpu.layer.set_view(queue, self.transform, [self.rect.width() * ppp, self.rect.height() * ppp]);
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, res: &egui_wgpu::CallbackResources) {
        let gpu: &Gpu = res.get().unwrap();
        gpu.renderer.draw(pass, &gpu.layer);
    }
}

/// Cache key: instances stay valid while the view is inside the cached window
/// at the same LOD level, so panning only updates the 32-byte uniform.
#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    level: usize,
    window: (usize, usize),
    origin: (f64, f64),
    show_ma: bool,
}

struct App {
    series: OhlcSeries,
    sma50: LineSeries,
    ema200: LineSeries,
    cam: Camera2D,
    cache: Option<CacheKey>,
    show_ma: bool,
    frame_ms: f64,
    frames: u64,
    last_build_ms: f64,
    uploads: u64,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rs = cc.wgpu_render_state.as_ref().expect("wgpu backend required");
        let renderer = Renderer::new(&rs.device, rs.target_format, 1);
        let layer = Layer::new(&rs.device, &renderer);
        rs.renderer.write().callback_resources.insert(Gpu { renderer, layer });
        let mut app = Self {
            series: OhlcSeries::default(),
            sma50: LineSeries::default(),
            ema200: LineSeries::default(),
            cam: Camera2D::new((0.0, 1.0), (0.0, 1.0), (1.0, 1.0)),
            cache: None,
            show_ma: true,
            frame_ms: 0.0,
            last_build_ms: 0.0,
            uploads: 0,
            frames: 0,
        };
        let n = std::env::var("PHOTIC_BARS").ok().and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
        app.load(n);
        if std::env::var("PHOTIC_VIEW").as_deref() == Ok("all") {
            app.cam.x = (0.0, n as f64);
        }
        app
    }

    fn load(&mut self, n: usize) {
        self.series = OhlcSeries::new(random_walk(n, 42));
        self.sma50 = LineSeries::new(&sma(self.series.bars(), 50));
        self.ema200 = LineSeries::new(&ema(self.series.bars(), 200));
        self.cam.x = (n.saturating_sub(400) as f64, n as f64 + 20.0);
        self.cache = None;
    }

    fn fit_y(&mut self) {
        let n = self.series.len();
        let s = self.cam.x.0.max(0.0) as usize;
        let e = (self.cam.x.1.max(0.0).ceil() as usize).min(n);
        if let Some((lo, hi)) = self.series.price_range(s.min(n.saturating_sub(1)), e.max(s + 1)) {
            let pad = (hi - lo).max(1e-9) * 0.08;
            self.cam.y = (lo - pad, hi + pad);
        }
    }

    fn build(&mut self, key: CacheKey) -> Instances {
        let mut inst = Instances::default();
        let style = CandleStyle::default();
        candles(&self.series, key.level, key.window, key.origin, &style, &mut inst.rects);
        if key.show_ma {
            line(&self.sma50, key.level, key.window, key.origin, Rgba::rgb(255, 193, 7), 1.5, &mut inst.segs);
            line(&self.ema200, key.level, key.window, key.origin, Rgba::rgb(171, 71, 188), 1.5, &mut inst.segs);
        }
        inst
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        let t0 = web_time_ms();
        egui::Panel::top("bar").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.strong("photic");
                for (label, n) in [("100K", 100_000), ("1M", 1_000_000), ("10M", 10_000_000)] {
                    if ui.selectable_label(self.series.len() == n, format!("{label} bars")).clicked() {
                        self.load(n);
                    }
                }
                ui.checkbox(&mut self.show_ma, "SMA50 / EMA200");
                ui.separator();
                ui.label(format!(
                    "visible {:.0} bars | build {:.2} ms | ui {:.2} ms | uploads {} | {:.0} fps",
                    self.cam.width(),
                    self.last_build_ms,
                    self.frame_ms,
                    self.uploads,
                    1.0 / ctx.input(|i| i.stable_dt).max(1e-6)
                ));
            });
        });

        egui::CentralPanel::default().frame(egui::Frame::new().fill(egui::Color32::from_rgb(18, 20, 26))).show(root, |ui| {
            let axis_w = 70.0;
            let axis_h = 22.0;
            let full = ui.available_rect_before_wrap();
            let plot = egui::Rect::from_min_max(full.min, egui::pos2(full.max.x - axis_w, full.max.y - axis_h));
            let resp = ui.allocate_rect(full, egui::Sense::drag());
            if plot.width() < 1.0 || plot.height() < 1.0 || self.series.is_empty() {
                return; // nothing visible: don't build (or mark cached) instances
            }
            let ppp = ctx.pixels_per_point() as f64;
            self.cam.size_px = (plot.width() as f64 * ppp, plot.height() as f64 * ppp);

            // --- input: drag = pan, wheel = zoom around cursor ---
            if resp.dragged() {
                let d = resp.drag_delta();
                self.cam.pan_px(d.x as f64 * ppp, 0.0);
            }
            if resp.hovered() {
                let scroll = ctx.input(|i| i.smooth_scroll_delta);
                let zoom = ctx.input(|i| i.zoom_delta());
                let mut factor = (-(scroll.y as f64) * 0.003).exp() / zoom as f64;
                if scroll.x != 0.0 {
                    self.cam.pan_px(scroll.x as f64 * ppp, 0.0);
                }
                if (factor - 1.0).abs() > 1e-6 {
                    let n = self.series.len() as f64;
                    let w = self.cam.width();
                    factor = factor.clamp(10.0 / w, (n * 1.2) / w);
                    let anchor = resp.hover_pos().map(|p| (p.x - plot.min.x) as f64 * ppp).unwrap_or(self.cam.size_px.0);
                    self.cam.zoom_x_at(factor, anchor);
                }
            }
            self.fit_y();

            // --- LOD + instance cache ---
            let n = self.series.len();
            let level = lod_level_for(self.cam.x_per_px(), 0.25, self.series.pyramid.max_level());
            let slot = 1usize << level;
            let vis = (self.cam.x.0.max(0.0) as usize, (self.cam.x.1.max(0.0).ceil() as usize).min(n));
            let fits = |k: &CacheKey| k.level == level && k.show_ma == self.show_ma && vis.0 >= k.window.0 && vis.1 <= k.window.1;
            let mut upload = None;
            if !self.cache.as_ref().is_some_and(fits) {
                let tb = web_time_ms();
                let w = vis.1.saturating_sub(vis.0).max(slot);
                let window = (vis.0.saturating_sub(w) / slot * slot, (vis.1 + w).min(n));
                let mid = self.series.price_range(vis.0.min(n - 1), vis.1.max(vis.0 + 1)).map(|r| r.0).unwrap_or(0.0);
                let key = CacheKey { level, window, origin: (window.0 as f64, mid), show_ma: self.show_ma };
                upload = Some(Arc::new(self.build(key)));
                self.cache = Some(key);
                self.last_build_ms = web_time_ms() - tb;
                self.uploads += 1;
            }
            let key = self.cache.unwrap();
            let painter = ui.painter_at(full);

            // grid + axes (egui text; geometry count is tiny)
            let grid = egui::Color32::from_gray(38);
            let text = egui::Color32::from_gray(150);
            let font = egui::FontId::monospace(11.0);
            let ystep = photic_core::nice_step(self.cam.height(), 8);
            let dec = (-ystep.log10().floor()).clamp(0.0, 10.0) as usize;
            for p in nice_ticks(self.cam.y.0, self.cam.y.1, 8) {
                let y = plot.min.y + (self.cam.to_px(0.0, p).1 / ppp) as f32;
                painter.hline(plot.x_range(), y, (1.0, grid));
                painter.text(egui::pos2(plot.max.x + 6.0, y), egui::Align2::LEFT_CENTER, format!("{p:.dec$}"), font.clone(), text);
            }
            let step = photic_core::nice_step(self.cam.width(), (plot.width() / 140.0).max(2.0) as usize).max(1.0);
            let mut i = (self.cam.x.0 / step).ceil() * step;
            while i <= self.cam.x.1 {
                if i >= 0.0 && (i as usize) < n {
                    let x = plot.min.x + (self.cam.to_px(i + 0.5, 0.0).0 / ppp) as f32;
                    painter.vline(x, plot.y_range(), (1.0, grid));
                    let t = self.series.time_at(i as usize).unwrap();
                    painter.text(egui::pos2(x, plot.max.y + 4.0), egui::Align2::CENTER_TOP, format_utc(t), font.clone(), text);
                }
                i += step;
            }

            // GPU layer
            let transform = self.cam.gpu_transform(key.origin);
            painter.add(egui_wgpu::Callback::new_paint_callback(plot, Frame { upload, transform, rect: plot }));

            // crosshair + OHLC readout
            if let Some(p) = resp.hover_pos().filter(|p| plot.contains(*p)) {
                let cross = egui::Color32::from_gray(110);
                painter.hline(plot.x_range(), p.y, (1.0, cross));
                painter.vline(p.x, plot.y_range(), (1.0, cross));
                let (dx, dy) = self.cam.from_px((p.x - plot.min.x) as f64 * ppp, (p.y - plot.min.y) as f64 * ppp);
                painter.text(egui::pos2(plot.max.x + 6.0, p.y), egui::Align2::LEFT_CENTER, format!("{dy:.*}", dec + 1), font.clone(), egui::Color32::WHITE);
                if dx >= 0.0 && (dx as usize) < n {
                    let lvl_slot = (dx as usize) / slot;
                    let b = self.series.pyramid.level(level)[lvl_slot.min(self.series.pyramid.level(level).len() - 1)];
                    painter.text(
                        plot.min + egui::vec2(8.0, 8.0),
                        egui::Align2::LEFT_TOP,
                        format!("{}  O {:.2}  H {:.2}  L {:.2}  C {:.2}  V {:.0}  (LOD {level}: {slot} bar/candle)", format_utc(b.time), b.open, b.high, b.low, b.close, b.volume),
                        font,
                        egui::Color32::from_gray(210),
                    );
                }
            }
        });
        self.frame_ms = web_time_ms() - t0;
        self.frames += 1;
        self.debug_screenshot(ctx);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn web_time_ms() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
fn web_time_ms() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1400.0, 800.0]).with_title("photic demo"),
        ..Default::default()
    };
    eframe::run_native("photic demo", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    std::panic::set_hook(Box::new(|p| web_sys::console::error_1(&format!("photic panic: {p}").into())));
    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("photic"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("canvas #photic");
        eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(App::new(cc)))))
            .await
            .expect("start");
    });
}

impl App {
    /// `PHOTIC_SHOT=out.ppm` saves a screenshot after a few frames and exits
    /// (used for automated visual checks).
    #[cfg(not(target_arch = "wasm32"))]
    fn debug_screenshot(&mut self, ctx: &egui::Context) {
        let Ok(path) = std::env::var("PHOTIC_SHOT") else { return };
        if self.frames == 30 {
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
            eprintln!("screenshot saved; build {:.3} ms, ui {:.3} ms", self.last_build_ms, self.frame_ms);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    #[cfg(target_arch = "wasm32")]
    fn debug_screenshot(&mut self, _ctx: &egui::Context) {}
}
