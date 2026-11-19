//! # photic-core
//!
//! Renderer-agnostic building blocks for high-performance data visualisation:
//!
//! * [`Pyramid`]: a generic level-of-detail (LOD) tree over any series whose
//!   elements can be merged ([`Aggregate`]). Range queries and "what to draw at
//!   this zoom" cost `O(log n + pixels)`, independent of the dataset size.
//! * [`scale`]: d3-style scales (linear, log, time, band) with "nice" ticks.
//! * [`Camera2D`]: pan/zoom state in `f64`, exported to the GPU as an `f32`
//!   affine transform relative to a local origin, so large values (timestamps,
//!   prices, coordinates) keep full precision on the GPU.
//!
//! Nothing here depends on a GPU or GUI toolkit; `photic-gpu` renders it with
//! wgpu, but a CPU rasteriser or SVG exporter could sit on top just as well.

pub mod camera;
pub mod pyramid;
pub mod scale;

pub use camera::{Camera2D, GpuTransform};
pub use pyramid::{Aggregate, MinMax, Pyramid};
pub use scale::{
    BandScale, LinearScale, LogScale, TimeScale, TimeTick, format_utc, nice_step, nice_ticks,
};

/// Pick the pyramid level so that roughly `target_per_px` items cover one
/// pixel. `items_per_px` is visible-items / plot-width-in-pixels.
pub fn lod_level_for(items_per_px: f64, target_per_px: f64, max_level: usize) -> usize {
    if !(items_per_px > target_per_px) {
        return 0;
    }
    ((items_per_px / target_per_px).log2().floor() as usize).min(max_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_level_selection() {
        assert_eq!(lod_level_for(0.1, 1.0, 20), 0);
        assert_eq!(lod_level_for(1.0, 1.0, 20), 0);
        assert_eq!(lod_level_for(2.0, 1.0, 20), 1);
        assert_eq!(lod_level_for(1000.0, 1.0, 20), 9);
        assert_eq!(lod_level_for(1e9, 1.0, 5), 5);
    }
}

pub mod prims;
pub use prims::{Point, Rect, Rgba, Segment};

pub mod heatmap;
pub use heatmap::{Heatmap, heat};
