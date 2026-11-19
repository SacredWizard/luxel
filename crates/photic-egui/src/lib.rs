//! # photic-egui
//!
//! A ready-made candlestick chart widget for [egui] on top of photic's GPU
//! renderer: candles, indicator lines, an order-book heatmap layer and a
//! volume pane, with pan/zoom, an eased auto-fitting price axis, axes and a
//! crosshair.
//!
//! ```ignore
//! // once, at startup (eframe with the wgpu renderer):
//! photic_egui::install(cc.wgpu_render_state.as_ref().unwrap());
//!
//! // every frame:
//! Chart::new("btc", &series)
//!     .line(&sma50, Rgba::rgb(255, 193, 7), 1.5)
//!     .heatmap(&book)
//!     .volume(true)
//!     .show(ui, &mut self.chart_state);
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, FontId, Pos2, Rect as ERect, Sense, Ui};
use egui_wgpu::wgpu;
use photic_core::{
    Camera2D, GpuTransform, Heatmap, Rect, Rgba, Segment, format_utc, heat, lod_level_for,
    nice_step, nice_ticks,
};
use photic_finance::{Bar, CandleStyle, LineSeries, OhlcSeries, candles, line, volume};
use photic_gpu::{Layer, Renderer};

/// Shared GPU state stored in egui-wgpu's callback resources.
struct Resources {
    renderer: Renderer,
    layers: HashMap<(egui::Id, u8), Layer>,
}

/// Register photic's GPU pipelines with egui-wgpu. Call once at startup.
pub fn install(rs: &egui_wgpu::RenderState) {
    let renderer = Renderer::new(&rs.device, rs.target_format, 1);
    rs.renderer.write().callback_resources.insert(Resources {
        renderer,
        layers: HashMap::new(),
    });
}

#[derive(Default)]
struct Instances {
    rects: Vec<Rect>,
    segs: Vec<Segment>,
}

struct PaneCallback {
    key: (egui::Id, u8),
    upload: Option<Arc<Instances>>,
    transform: GpuTransform,
    size_points: egui::Vec2,
}

impl egui_wgpu::CallbackTrait for PaneCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _enc: &mut wgpu::CommandEncoder,
        res: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(r) = res.get_mut::<Resources>() else {
            return Vec::new();
        };
        let Resources { renderer, layers } = r;
        let layer = layers
            .entry(self.key)
            .or_insert_with(|| Layer::new(device, renderer));
        if let Some(inst) = &self.upload {
            layer.set_rects(device, queue, &inst.rects);
            layer.set_segments(device, queue, &inst.segs);
        }
        let ppp = screen.pixels_per_point;
        layer.set_view(
            queue,
            self.transform,
            [self.size_points.x * ppp, self.size_points.y * ppp],
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        res: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = res.get::<Resources>()
            && let Some(layer) = r.layers.get(&self.key)
        {
            r.renderer.draw(pass, layer);
        }
    }
}

/// Visual options.
#[derive(Clone, Debug)]
pub struct ChartStyle {
    pub candles: CandleStyle,
    pub background: Color32,
    pub grid: Color32,
    pub text: Color32,
    pub crosshair: Color32,
    /// Time constant (seconds) of the price-axis easing; 0 disables easing.
    pub y_ease_secs: f32,
    /// Fraction of the plot height given to the volume pane.
    pub volume_fraction: f32,
    /// Heatmap cells below this normalised intensity are not drawn.
    pub heatmap_threshold: f32,
    /// Candles per pixel before switching to a coarser LOD level.
    pub candles_per_px: f64,
}

impl Default for ChartStyle {
    fn default() -> Self {
        Self {
            candles: CandleStyle::default(),
            background: Color32::from_rgb(18, 20, 26),
            grid: Color32::from_gray(38),
            text: Color32::from_gray(150),
            crosshair: Color32::from_gray(110),
            y_ease_secs: 0.08,
            volume_fraction: 0.2,
            heatmap_threshold: 0.12,
            candles_per_px: 0.25,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CacheKey {
    level: usize,
    window: (usize, usize),
    origin: (f64, f64),
    /// Identity of the data/options that went into the instances.
    content: u64,
    /// Heatmap row bucketing (depends on zoom in y).
    row_step: usize,
}

/// Per-chart state that persists across frames.
#[derive(Clone)]
pub struct ChartState {
    pub cam: Camera2D,
    /// Displayed (eased) price range; `cam.y` is snapped to it each frame.
    y_shown: Option<(f64, f64)>,
    vol_shown: Option<f64>,
    cache: Option<CacheKey>,
    /// Bump to force rebuilding instances (e.g. after the data changed).
    pub generation: u64,
    /// Stats of the last frame.
    pub stats: FrameStats,
    /// Whether the camera has been positioned (by the widget or the app).
    placed: bool,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            cam: Camera2D::new((0.0, 1.0), (0.0, 1.0), (1.0, 1.0)),
            y_shown: None,
            vol_shown: None,
            cache: None,
            generation: 0,
            stats: FrameStats::default(),
            placed: false,
        }
    }
}

impl ChartState {
    /// Show the last `n` bars (plus a little right margin).
    pub fn show_last(&mut self, total: usize, n: usize) {
        self.cam.x = (
            total.saturating_sub(n) as f64,
            total as f64 + (n as f64 * 0.05).max(2.0),
        );
        self.y_shown = None; // snap on next frame
        self.vol_shown = None;
        self.placed = true;
    }

    /// Show the whole series.
    pub fn show_all(&mut self, total: usize) {
        self.cam.x = (0.0, total.max(1) as f64);
        self.y_shown = None;
        self.vol_shown = None;
        self.placed = true;
    }

    /// Invalidate cached GPU instances (call after mutating the data).
    pub fn data_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub lod_level: usize,
    pub instances: usize,
    pub rebuilt: bool,
    pub build_ms: f64,
    pub rebuilds: u64,
}

/// What happened this frame.
pub struct ChartResponse {
    pub response: egui::Response,
    /// The (possibly aggregated) bar under the cursor.
    pub hovered: Option<Bar>,
}

/// The chart widget (builder).
pub struct Chart<'a> {
    id: egui::Id,
    series: &'a OhlcSeries,
    lines: Vec<(&'a LineSeries, Rgba, f32)>,
    heatmap: Option<&'a Heatmap>,
    volume: bool,
    style: ChartStyle,
}

impl<'a> Chart<'a> {
    pub fn new(id_salt: impl egui::AsId, series: &'a OhlcSeries) -> Self {
        Self {
            id: egui::Id::new(id_salt),
            series,
            lines: Vec::new(),
            heatmap: None,
            volume: false,
            style: ChartStyle::default(),
        }
    }

    pub fn line(mut self, line: &'a LineSeries, color: Rgba, width_px: f32) -> Self {
        self.lines.push((line, color, width_px));
        self
    }

    pub fn heatmap(mut self, heatmap: &'a Heatmap) -> Self {
        self.heatmap = Some(heatmap);
        self
    }

    pub fn volume(mut self, on: bool) -> Self {
        self.volume = on;
        self
    }

    pub fn style(mut self, style: ChartStyle) -> Self {
        self.style = style;
        self
    }

    pub fn show(self, ui: &mut Ui, state: &mut ChartState) -> ChartResponse {
        let n = self.series.len();
        let style = &self.style;
        let full = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(full, Sense::click_and_drag());
        if n == 0 || full.width() < 80.0 || full.height() < 60.0 {
            return ChartResponse {
                response,
                hovered: None,
            };
        }
        if !state.placed {
            state.show_last(n, 300);
        }

        // --- layout ---
        let (axis_w, axis_h, gap) = (70.0, 22.0, 4.0);
        let body = ERect::from_min_max(
            full.min,
            Pos2::new(full.max.x - axis_w, full.max.y - axis_h),
        );
        let vol_h = if self.volume {
            (body.height() * style.volume_fraction).round()
        } else {
            0.0
        };
        let price = ERect::from_min_max(
            body.min,
            Pos2::new(
                body.max.x,
                body.max.y - if self.volume { vol_h + gap } else { 0.0 },
            ),
        );
        let vol = ERect::from_min_max(Pos2::new(body.min.x, body.max.y - vol_h), body.max);
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point() as f64;
        state.cam.size_px = (price.width() as f64 * ppp, price.height() as f64 * ppp);

        // --- input: drag = pan, wheel/pinch = zoom at cursor, double-click = reset ---
        if response.dragged() {
            state.cam.pan_px(response.drag_delta().x as f64 * ppp, 0.0);
        }
        if response.double_clicked() {
            state.show_last(n, 300);
        }
        if response.hovered() {
            let (scroll, zoom) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if scroll.x != 0.0 {
                state.cam.pan_px(scroll.x as f64 * ppp, 0.0);
            }
            let mut factor = (-(scroll.y as f64) * 0.003).exp() / zoom as f64;
            if (factor - 1.0).abs() > 1e-6 {
                let w = state.cam.width();
                factor = factor.clamp(10.0 / w, (n as f64 * 1.2).max(10.0) / w);
                let anchor = response
                    .hover_pos()
                    .map(|p| (p.x - price.min.x) as f64 * ppp)
                    .unwrap_or(state.cam.size_px.0);
                state.cam.zoom_x_at(factor, anchor);
            }
        }
        // keep some data on screen
        let w = state.cam.width();
        if state.cam.x.1 < 1.0 {
            state.cam.x = (1.0 - w, 1.0);
        } else if state.cam.x.0 > n as f64 - 1.0 {
            state.cam.x = (n as f64 - 1.0, n as f64 - 1.0 + w);
        }

        // --- visible range, LOD level ---
        let vis = (
            state.cam.x.0.max(0.0) as usize,
            (state.cam.x.1.max(0.0).ceil() as usize).min(n),
        );
        let vis = (vis.0.min(n - 1), vis.1.max(vis.0.min(n - 1) + 1));
        let level = lod_level_for(
            state.cam.x_per_px(),
            style.candles_per_px,
            self.series.pyramid.max_level(),
        );
        let slot = 1usize << level;

        // --- eased price axis (the fix for the "bounce" when zooming) ---
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) as f64;
        let ease = if style.y_ease_secs > 0.0 {
            1.0 - (-dt / style.y_ease_secs as f64).exp()
        } else {
            1.0
        };
        let mut animating = false;
        if let Some((lo, hi)) = self.series.price_range(vis.0, vis.1) {
            let pad = (hi - lo).max(hi.abs() * 1e-6).max(1e-9) * 0.08;
            let target = (lo - pad, hi + pad);
            let shown = match state.y_shown {
                Some(s) => {
                    let next = (s.0 + (target.0 - s.0) * ease, s.1 + (target.1 - s.1) * ease);
                    let eps = (target.1 - target.0) * 1e-4;
                    if (next.0 - target.0).abs() < eps && (next.1 - target.1).abs() < eps {
                        target
                    } else {
                        animating = true;
                        next
                    }
                }
                None => target,
            };
            state.y_shown = Some(shown);
            state.cam.y = shown;
        }

        // --- instance cache: rebuild only when level/window/content changes ---
        let content = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (
                state.generation,
                n,
                self.volume,
                self.lines.len(),
                self.heatmap.map(|h| h.cols()),
            )
                .hash(&mut h);
            for (l, c, w) in &self.lines {
                (l.pyramid.len(), c.0, w.to_bits()).hash(&mut h);
            }
            h.finish()
        };
        let row_step = self.heatmap.map_or(1, |hm| {
            let rows_per_px = (state.cam.height() / hm.dy) / state.cam.size_px.1.max(1.0);
            (rows_per_px / 0.5).ceil().max(1.0) as usize
        });
        let fits = |k: &CacheKey| {
            k.level == level
                && k.content == content
                && k.row_step == row_step
                && vis.0 >= k.window.0
                && vis.1 <= k.window.1
        };
        let mut uploads: [Option<Arc<Instances>>; 2] = [None, None];
        let mut vol_max = None;
        let rebuilt = !state.cache.as_ref().is_some_and(fits);
        if rebuilt {
            let t0 = now_ms();
            let span = vis.1 - vis.0;
            let window = (
                vis.0.saturating_sub(span) / slot * slot,
                (vis.1 + span).min(n),
            );
            let base = self.series.price_range(vis.0, vis.1).map_or(0.0, |r| r.0);
            let key = CacheKey {
                level,
                window,
                origin: (window.0 as f64, base),
                content,
                row_step,
            };
            let mut price_inst = Instances::default();
            if let Some(hm) = self.heatmap {
                let y = (f64::NEG_INFINITY, f64::INFINITY);
                hm.cells(
                    level,
                    (window.0 as f64, window.1 as f64),
                    y,
                    row_step,
                    key.origin,
                    style.heatmap_threshold,
                    heat,
                    &mut price_inst.rects,
                );
            }
            candles(
                self.series,
                level,
                window,
                key.origin,
                &style.candles,
                &mut price_inst.rects,
            );
            for (l, c, w) in &self.lines {
                line(l, level, window, key.origin, *c, *w, &mut price_inst.segs);
            }
            let mut vol_inst = Instances::default();
            if self.volume {
                volume(
                    self.series,
                    level,
                    window,
                    key.origin.0,
                    &style.candles,
                    150,
                    &mut vol_inst.rects,
                );
            }
            state.stats.instances =
                price_inst.rects.len() + price_inst.segs.len() + vol_inst.rects.len();
            uploads = [Some(Arc::new(price_inst)), Some(Arc::new(vol_inst))];
            state.cache = Some(key);
            state.stats.build_ms = now_ms() - t0;
            state.stats.rebuilds += 1;
        }
        let key = state.cache.unwrap();
        state.stats.lod_level = level;
        state.stats.rebuilt = rebuilt;

        // volume pane fit (visible bars only), eased like the price axis
        if self.volume {
            let (_, items) = self.series.pyramid.visible(level, vis.0, vis.1);
            let target = items.iter().fold(0.0f64, |m, b| m.max(b.volume)).max(1e-9) * 1.1;
            let shown = match state.vol_shown {
                Some(s) => {
                    let next = s + (target - s) * ease;
                    if (next - target).abs() > target * 1e-4 {
                        animating = true;
                        next
                    } else {
                        target
                    }
                }
                None => target,
            };
            state.vol_shown = Some(shown);
            vol_max = Some(shown);
        }
        if animating {
            ctx.request_repaint();
        }

        // --- background, grid, axes ---
        let painter = ui.painter_at(full);
        painter.rect_filled(body, 0.0, style.background);
        let font = FontId::monospace(11.0);
        let ystep = nice_step(state.cam.height(), 8);
        let dec = (-ystep.log10().floor()).clamp(0.0, 10.0) as usize;
        for p in nice_ticks(state.cam.y.0, state.cam.y.1, 8) {
            let y = price.min.y + (state.cam.to_px(0.0, p).1 / ppp) as f32;
            painter.hline(price.x_range(), y, (1.0, style.grid));
            painter.text(
                Pos2::new(price.max.x + 6.0, y),
                Align2::LEFT_CENTER,
                format!("{p:.dec$}"),
                font.clone(),
                style.text,
            );
        }
        let xstep =
            nice_step(state.cam.width(), (price.width() / 140.0).max(2.0) as usize).max(1.0);
        let mut i = (state.cam.x.0 / xstep).ceil() * xstep;
        while i <= state.cam.x.1 {
            if i >= 0.0 && (i as usize) < n {
                let x = price.min.x + (state.cam.to_px(i + 0.5, 0.0).0 / ppp) as f32;
                painter.vline(x, body.y_range(), (1.0, style.grid));
                let t = self.series.time_at(i as usize).unwrap_or_default();
                painter.text(
                    Pos2::new(x, body.max.y + 4.0),
                    Align2::CENTER_TOP,
                    format_utc(t),
                    font.clone(),
                    style.text,
                );
            }
            i += xstep;
        }
        if self.volume {
            painter.hline(body.x_range(), vol.min.y - gap * 0.5, (1.0, style.grid));
        }

        // --- GPU panes ---
        let [up_price, up_vol] = uploads;
        let t_price = state.cam.gpu_transform(key.origin);
        painter.add(egui_wgpu::Callback::new_paint_callback(
            price,
            PaneCallback {
                key: (self.id, 0),
                upload: up_price,
                transform: t_price,
                size_points: price.size(),
            },
        ));
        if let Some(vmax) = vol_max {
            let vcam = Camera2D::new(
                state.cam.x,
                (0.0, vmax),
                (vol.width() as f64 * ppp, vol.height() as f64 * ppp),
            );
            painter.add(egui_wgpu::Callback::new_paint_callback(
                vol,
                PaneCallback {
                    key: (self.id, 1),
                    upload: up_vol,
                    transform: vcam.gpu_transform((key.origin.0, 0.0)),
                    size_points: vol.size(),
                },
            ));
        }

        // --- crosshair + OHLC readout ---
        let mut hovered = None;
        if let Some(p) = response.hover_pos().filter(|p| body.contains(*p)) {
            painter.hline(body.x_range(), p.y, (1.0, style.crosshair));
            painter.vline(p.x, body.y_range(), (1.0, style.crosshair));
            let (dx, dy) = state.cam.from_px(
                (p.x - price.min.x) as f64 * ppp,
                (p.y - price.min.y) as f64 * ppp,
            );
            if price.contains(p) {
                painter.text(
                    Pos2::new(price.max.x + 6.0, p.y),
                    Align2::LEFT_CENTER,
                    format!("{dy:.*}", dec + 1),
                    font.clone(),
                    Color32::WHITE,
                );
            }
            if dx >= 0.0 && (dx as usize) < n {
                let lvl = self.series.pyramid.level(level);
                let b = lvl[((dx as usize) / slot).min(lvl.len() - 1)];
                painter.text(
                    price.min + egui::vec2(8.0, 8.0),
                    Align2::LEFT_TOP,
                    format!(
                        "{}  O {:.*}  H {:.*}  L {:.*}  C {:.*}  V {:.0}",
                        format_utc(b.time),
                        dec + 1,
                        b.open,
                        dec + 1,
                        b.high,
                        dec + 1,
                        b.low,
                        dec + 1,
                        b.close,
                        b.volume
                    ),
                    font,
                    Color32::from_gray(210),
                );
                hovered = Some(b);
            }
        }
        ChartResponse { response, hovered }
    }
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
