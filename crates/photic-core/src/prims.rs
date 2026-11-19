//! GPU-ready primitive instances. Every chart type (candles, bars, heatmaps,
//! scatter, lines, areas) is lowered into these three, so one renderer serves
//! all of them. Coordinates are `f32` data units relative to a layer origin
//! (see [`crate::Camera2D::gpu_transform`]).

use bytemuck::{Pod, Zeroable};

/// Packed RGBA8 colour (`0xRRGGBBAA`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Rgba(pub u32);

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | a as u32)
    }
}

/// Axis-aligned rectangle (candle body, bar, heatmap cell, wick).
/// `min_px` enforces a minimum on-screen size around the rect centre, so a
/// 1px wick or a zero-height doji never disappears at any zoom.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Rect {
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub min_px: [f32; 2],
    pub color: Rgba,
    pub _pad: u32,
}

impl Rect {
    pub fn new(min: [f32; 2], max: [f32; 2], color: Rgba) -> Self {
        Self {
            min,
            max,
            min_px: [0.0, 0.0],
            color,
            _pad: 0,
        }
    }
    pub fn with_min_px(mut self, w: f32, h: f32) -> Self {
        self.min_px = [w, h];
        self
    }
}

/// Line segment with a constant pixel width (polylines, indicators, grid).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Segment {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub color: Rgba,
    pub width_px: f32,
}

/// Round point with a pixel radius (scatter plots, markers).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct Point {
    pub pos: [f32; 2],
    pub color: Rgba,
    pub radius_px: f32,
}

/// Turn a polyline into segments.
pub fn polyline(points: &[[f32; 2]], color: Rgba, width_px: f32, out: &mut Vec<Segment>) {
    out.extend(points.windows(2).map(|w| Segment {
        a: w[0],
        b: w[1],
        color,
        width_px,
    }));
}
