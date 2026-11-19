//! 2D camera: the visible data window, kept in `f64`.

/// Affine transform for the GPU: `clip = (data - origin) * scale + offset`.
/// Data is uploaded to the GPU as `f32(value - origin)`, so precision is
/// relative to the origin, not to zero (timestamps, prices, lat/lon all work).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuTransform {
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera2D {
    /// Visible data range on x: `[x0, x1]`.
    pub x: (f64, f64),
    /// Visible data range on y: `[y0, y1]` (y0 at the bottom).
    pub y: (f64, f64),
    /// Plot size in physical pixels.
    pub size_px: (f64, f64),
}

impl Camera2D {
    pub fn new(x: (f64, f64), y: (f64, f64), size_px: (f64, f64)) -> Self {
        Self { x, y, size_px }
    }

    pub fn width(&self) -> f64 {
        self.x.1 - self.x.0
    }

    pub fn height(&self) -> f64 {
        self.y.1 - self.y.0
    }

    /// Data units per pixel on x.
    pub fn x_per_px(&self) -> f64 {
        self.width() / self.size_px.0.max(1.0)
    }

    pub fn y_per_px(&self) -> f64 {
        self.height() / self.size_px.1.max(1.0)
    }

    /// Data -> pixel (top-left origin).
    pub fn to_px(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.x.0) / self.width() * self.size_px.0,
            (self.y.1 - y) / self.height() * self.size_px.1,
        )
    }

    /// Pixel -> data.
    pub fn from_px(&self, px: f64, py: f64) -> (f64, f64) {
        (
            self.x.0 + px / self.size_px.0 * self.width(),
            self.y.1 - py / self.size_px.1 * self.height(),
        )
    }

    /// Pan by a pixel delta (drag right => content moves right).
    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        let ddx = -dx * self.x_per_px();
        let ddy = dy * self.y_per_px();
        self.x = (self.x.0 + ddx, self.x.1 + ddx);
        self.y = (self.y.0 + ddy, self.y.1 + ddy);
    }

    /// Zoom x around a pixel anchor. `factor < 1` zooms in.
    pub fn zoom_x_at(&mut self, factor: f64, anchor_px: f64) {
        let (ax, _) = self.from_px(anchor_px, 0.0);
        self.x = (ax + (self.x.0 - ax) * factor, ax + (self.x.1 - ax) * factor);
    }

    pub fn zoom_y_at(&mut self, factor: f64, anchor_py: f64) {
        let (_, ay) = self.from_px(0.0, anchor_py);
        self.y = (ay + (self.y.0 - ay) * factor, ay + (self.y.1 - ay) * factor);
    }

    /// GPU transform for data uploaded relative to `origin`.
    pub fn gpu_transform(&self, origin: (f64, f64)) -> GpuTransform {
        let sx = 2.0 / self.width();
        let sy = 2.0 / self.height();
        GpuTransform {
            scale: [sx as f32, sy as f32],
            offset: [
                ((origin.0 - self.x.0) * sx - 1.0) as f32,
                ((origin.1 - self.y.0) * sy - 1.0) as f32,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_roundtrip_and_zoom_anchor() {
        let mut c = Camera2D::new((1.6e12, 1.6e12 + 3.6e6), (100.0, 200.0), (800.0, 600.0));
        let (x, y) = c.from_px(200.0, 150.0);
        let (px, py) = c.to_px(x, y);
        assert!((px - 200.0).abs() < 1e-6 && (py - 150.0).abs() < 1e-6);
        let before = c.from_px(200.0, 0.0).0;
        c.zoom_x_at(0.5, 200.0);
        assert!((c.from_px(200.0, 0.0).0 - before).abs() < 1e-3);
    }

    #[test]
    fn gpu_transform_maps_corners() {
        let c = Camera2D::new((1.6e12, 1.6e12 + 1000.0), (10.0, 20.0), (100.0, 100.0));
        let origin = (1.6e12, 10.0);
        let t = c.gpu_transform(origin);
        let clip =
            |dx: f32, dy: f32| (dx * t.scale[0] + t.offset[0], dy * t.scale[1] + t.offset[1]);
        let (a, b) = clip(0.0, 0.0);
        let (cx, cy) = clip(1000.0, 10.0);
        assert!((a + 1.0).abs() < 1e-6 && (b + 1.0).abs() < 1e-6);
        assert!((cx - 1.0).abs() < 1e-6 && (cy - 1.0).abs() < 1e-6);
    }
}
