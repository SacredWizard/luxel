//! # photic-egui
//!
//! A ready-made candlestick chart widget for [egui] on top of photic's GPU
//! renderer: candles, indicator lines, an order-book heatmap layer and a
//! volume pane, with pan/zoom, a smoothly auto-fitting price axis, axes and
//! a crosshair.
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
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Align2, Color32, FontId, Pos2, Rect as ERect, Sense, Ui};
use egui_wgpu::wgpu;
use photic_core::{
    Camera2D, GpuTransform, Heatmap, Rect, Rgba, Segment, format_utc, heat, lod_level_for,
    nice_step, nice_ticks,
};
use photic_finance::{Bar, CandleStyle, LineSeries, OhlcSeries, avg_volume, candles, line, volume};
use photic_gpu::{Layer, Renderer};

/// Shared GPU state stored in egui-wgpu's callback resources.
struct Resources {
    renderer: Renderer,
    /// Layer per (chart, pane) plus the stamp of the instances it holds.
    layers: HashMap<(egui::Id, u8), (Layer, u64)>,
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

/// One GPU pane. It always carries the latest instances and their stamp;
/// `prepare` uploads only when the layer holds an older stamp, so an upload
/// can never be lost (e.g. when a frame's callback is culled).
struct PaneCallback {
    key: (egui::Id, u8),
    instances: Arc<Instances>,
    stamp: u64,
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
        let (layer, stamp) = layers
            .entry(self.key)
            .or_insert_with(|| (Layer::new(device, renderer), u64::MAX));
        if *stamp != self.stamp {
            layer.set_rects(device, queue, &self.instances.rects);
            layer.set_segments(device, queue, &self.instances.segs);
            *stamp = self.stamp;
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
            && let Some((layer, _)) = r.layers.get(&self.key)
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
    /// Response time (seconds) of the auto-fitting price/volume axes: a
    /// critically damped spring that follows the visible range smoothly
    /// without overshoot. 0 snaps instantly.
    pub y_smooth_secs: f32,
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
            y_smooth_secs: 0.12,
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
    /// Identity of the data and options that went into the instances.
    content: u64,
    /// Heatmap row bucketing (depends on zoom in y).
    row_step: usize,
    /// y window the heatmap was culled to.
    y_window: (f64, f64),
}

/// A critically damped spring (value + velocity) for smooth axis fitting.
#[derive(Clone, Copy, Debug, Default)]
struct Spring {
    x: f64,
    v: f64,
}

impl Spring {
    /// Advance toward `target` by `dt` seconds with response time `t`.
    /// Returns true while still moving. Exact for a constant target, so it
    /// is stable at any frame rate and never overshoots.
    fn step(&mut self, target: f64, dt: f64, t: f64, eps: f64) -> bool {
        if t <= 0.0 {
            *self = Spring { x: target, v: 0.0 };
            return false;
        }
        let w = 2.0 / t;
        let e = self.x - target;
        let decay = (-w * dt).exp();
        let tmp = (self.v + w * e) * dt;
        let e2 = (e + tmp) * decay;
        self.v = (self.v - w * tmp) * decay;
        self.x = target + e2;
        if e2.abs() < eps && self.v.abs() * 0.016 < eps {
            *self = Spring { x: target, v: 0.0 };
            false
        } else {
            true
        }
    }
}

/// Per-chart state that persists across frames.
#[derive(Clone)]
pub struct ChartState {
    pub cam: Camera2D,
    y_lo: Option<Spring>,
    y_hi: Spring,
    vol: Option<Spring>,
    cache: Option<CacheKey>,
    instances: [Arc<Instances>; 2],
    stamp: u64,
    /// Bump (via [`ChartState::data_changed`]) to force rebuilding instances.
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
            y_lo: None,
            y_hi: Spring::default(),
            vol: None,
            cache: None,
            instances: [Arc::default(), Arc::default()],
            stamp: 0,
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
        self.snap_axes();
    }

    /// Show the whole series.
    pub fn show_all(&mut self, total: usize) {
        self.cam.x = (0.0, total.max(1) as f64);
        self.snap_axes();
    }

    fn snap_axes(&mut self) {
        self.y_lo = None; // fit instantly on the next frame
        self.vol = None;
        self.placed = true;
    }

    /// Invalidate cached GPU instances. Call after mutating series, lines or
    /// the heatmap in place (appends that change the length are detected).
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

    /// Hash of everything that goes into the instances besides the view.
    fn content_hash(&self, generation: u64) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let s = &self.style;
        (generation, self.series.len(), self.volume).hash(&mut h);
        (s.candles.up.0, s.candles.down.0, s.candles.body.to_bits()).hash(&mut h);
        s.heatmap_threshold.to_bits().hash(&mut h);
        (std::ptr::from_ref(self.series) as usize).hash(&mut h);
        for (l, c, w) in &self.lines {
            (
                std::ptr::from_ref(*l) as usize,
                l.pyramid.len(),
                c.0,
                w.to_bits(),
            )
                .hash(&mut h);
        }
        if let Some(hm) = self.heatmap {
            let id = (
                std::ptr::from_ref(hm) as usize,
                hm.rows(),
                hm.cols(),
                hm.x0(),
            );
            (
                id,
                hm.y0.to_bits(),
                hm.dy.to_bits(),
                hm.max_value().to_bits(),
            )
                .hash(&mut h);
        }
        h.finish()
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
            let factor = (-(scroll.y as f64) * 0.003).exp() / zoom as f64;
            if (factor - 1.0).abs() > 1e-6 {
                // clamp the resulting width (never turn a zoom-out into a zoom-in)
                let w = state.cam.width();
                let min_w = 10.0f64.min(w);
                let max_w = (n as f64 * 1.2).max(10.0).max(w);
                let factor = (w * factor).clamp(min_w, max_w) / w;
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
        let last = n - 1;
        let vis = (
            (state.cam.x.0.max(0.0) as usize).min(last),
            (state.cam.x.1.max(0.0).ceil() as usize).min(n),
        );
        let vis = (vis.0, vis.1.max(vis.0 + 1));
        // bars whose centre is on screen (for fitting; partly visible bars
        // at the edges would make the axis jump as they slide in and out)
        let fit = {
            let s = ((state.cam.x.0 - 0.5).ceil().max(0.0) as usize).min(last);
            let e = (((state.cam.x.1 - 0.5).floor() + 1.0).max(0.0) as usize).min(n);
            (s, e.max(s + 1))
        };
        let level = lod_level_for(
            state.cam.x_per_px(),
            style.candles_per_px,
            self.series.pyramid.max_level(),
        );
        let slot = 1usize << level;

        // --- smooth price axis (spring toward the visible range) ---
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1) as f64;
        let t = style.y_smooth_secs as f64;
        let mut animating = false;
        if let Some((lo, hi)) = self.series.price_range(fit.0, fit.1) {
            let pad = (hi - lo).max(hi.abs() * 1e-6).max(1e-9) * 0.08;
            let target = (lo - pad, hi + pad);
            let eps = (target.1 - target.0) * 1e-4;
            match &mut state.y_lo {
                Some(y_lo) => {
                    animating |= y_lo.step(target.0, dt, t, eps);
                    animating |= state.y_hi.step(target.1, dt, t, eps);
                }
                None => {
                    state.y_lo = Some(Spring {
                        x: target.0,
                        v: 0.0,
                    });
                    state.y_hi = Spring {
                        x: target.1,
                        v: 0.0,
                    };
                }
            }
            let (a, b) = (state.y_lo.unwrap().x, state.y_hi.x);
            state.cam.y = if b > a { (a, b) } else { target };
        }

        // --- instance cache: rebuild only when level/window/content changes ---
        let content = self.content_hash(state.generation);
        let row_step = self.heatmap.map_or(1, |hm| {
            let rows_per_px = (state.cam.height() / hm.dy) / state.cam.size_px.1.max(1.0);
            (rows_per_px / 0.5).ceil().max(1.0) as usize
        });
        let vis_y = state.cam.y;
        let fits = |k: &CacheKey| {
            k.level == level
                && k.content == content
                && k.row_step == row_step
                && vis.0 >= k.window.0
                && vis.1 <= k.window.1
                && vis_y.0 >= k.y_window.0
                && vis_y.1 <= k.y_window.1
        };
        let rebuilt = !state.cache.as_ref().is_some_and(fits);
        if rebuilt {
            let t0 = now_ms();
            let span = vis.1 - vis.0;
            let window = (
                vis.0.saturating_sub(span) / slot * slot,
                (vis.1 + span).min(n),
            );
            // cull the heatmap to 3x the visible price span (rebuilt when the
            // view leaves it); without a heatmap y never invalidates the cache
            let y_window = if self.heatmap.is_some() {
                let h = vis_y.1 - vis_y.0;
                (vis_y.0 - h, vis_y.1 + h)
            } else {
                (f64::NEG_INFINITY, f64::INFINITY)
            };
            let base = self.series.price_range(vis.0, vis.1).map_or(0.0, |r| r.0);
            let key = CacheKey {
                level,
                window,
                origin: (window.0 as f64, base),
                content,
                row_step,
                y_window,
            };
            let mut price_inst = Instances::default();
            if let Some(hm) = self.heatmap {
                let xr = (window.0 as f64, window.1 as f64);
                hm.cells(
                    level,
                    xr,
                    y_window,
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
            state.instances = [Arc::new(price_inst), Arc::new(vol_inst)];
            state.stamp = state.stamp.wrapping_add(1);
            state.cache = Some(key);
            state.stats.build_ms = now_ms() - t0;
            state.stats.rebuilds += 1;
        }
        let key = state.cache.unwrap();
        state.stats.lod_level = level;
        state.stats.rebuilt = rebuilt;

        // volume pane fit: per-bar volume of bars on screen, same spring
        let mut vol_max = None;
        if self.volume {
            let (first, items) = self.series.pyramid.visible(level, fit.0, fit.1);
            let target = items
                .iter()
                .enumerate()
                .map(|(i, b)| avg_volume(self.series, level, first, i, b))
                .fold(0.0f64, f64::max)
                .max(1e-9)
                * 1.1;
            let spring = state.vol.get_or_insert(Spring { x: target, v: 0.0 });
            animating |= spring.step(target, dt, t, target * 1e-4);
            vol_max = Some(spring.x.max(1e-9));
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
        painter.add(egui_wgpu::Callback::new_paint_callback(
            price,
            PaneCallback {
                key: (self.id, 0),
                instances: state.instances[0].clone(),
                stamp: state.stamp,
                transform: state.cam.gpu_transform(key.origin),
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
                    instances: state.instances[1].clone(),
                    stamp: state.stamp,
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
                let d = dec + 1;
                painter.text(
                    price.min + egui::vec2(8.0, 8.0),
                    Align2::LEFT_TOP,
                    format!(
                        "{}  O {:.d$}  H {:.d$}  L {:.d$}  C {:.d$}  V {:.0}",
                        format_utc(b.time),
                        b.open,
                        b.high,
                        b.low,
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

#[cfg(test)]
mod tests {
    use super::Spring;

    #[test]
    fn spring_settles_without_overshoot() {
        let mut s = Spring { x: 0.0, v: 0.0 };
        let mut prev = 0.0;
        let mut frames = 0;
        while s.step(100.0, 1.0 / 120.0, 0.12, 0.01) {
            assert!(s.x <= 100.0 + 1e-9, "overshoot at {}", s.x);
            assert!(s.x >= prev - 1e-9, "not monotonic");
            prev = s.x;
            frames += 1;
            assert!(frames < 240, "did not settle within 2 s");
        }
        assert_eq!(s.x, 100.0);
    }

    #[test]
    fn spring_is_frame_rate_independent() {
        let run = |hz: f64| {
            let mut s = Spring { x: 0.0, v: 0.0 };
            for _ in 0..(hz * 0.1) as usize {
                s.step(1.0, 1.0 / hz, 0.12, 1e-9);
            }
            s.x
        };
        assert!((run(60.0) - run(240.0)).abs() < 1e-6);
    }
}
