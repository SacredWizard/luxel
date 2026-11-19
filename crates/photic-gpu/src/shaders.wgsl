// photic-gpu shaders. One uniform per layer: data -> clip affine transform
// plus the viewport size, so pan/zoom never re-uploads instance data.

struct View {
    scale: vec2<f32>,
    offset: vec2<f32>,
    viewport_px: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(0) var<uniform> view: View;

fn to_px(p: vec2<f32>) -> vec2<f32> {
    let clip = p * view.scale + view.offset;
    return (clip * 0.5 + 0.5) * view.viewport_px;
}

fn px_to_clip(px: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px / view.viewport_px * 2.0 - 1.0, 0.0, 1.0);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
}

fn unpack(c: u32) -> vec4<f32> {
    let v = vec4<f32>(
        f32((c >> 24u) & 255u), f32((c >> 16u) & 255u),
        f32((c >> 8u) & 255u), f32(c & 255u)) / 255.0;
    // colours are sRGB bytes; an *Srgb target re-encodes, so linearise first
    if (SRGB_TARGET) {
        return vec4(srgb_to_linear(v.rgb), v.a);
    }
    return v;
}

fn corner(vi: u32) -> vec2<f32> {
    var c = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    return c[vi];
}

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) radius_px: f32,
};

// Snap a [lo, hi] pixel span to the pixel grid, enforcing a minimum size.
fn snap_span(lo_in: f32, hi_in: f32, min_px: f32) -> vec2<f32> {
    var lo = min(lo_in, hi_in);
    var hi = max(lo_in, hi_in);
    if (hi - lo < min_px) {
        let c = (lo + hi) * 0.5;
        lo = c - min_px * 0.5;
        hi = c + min_px * 0.5;
    }
    // floor(x + 0.5): ties always go the same way (round() is half-to-even)
    let s0 = floor(lo + 0.5);
    let s1 = max(floor(hi + 0.5), s0 + floor(max(min_px, 0.0) + 0.5));
    return vec2(s0, s1);
}

@vertex
fn vs_rect(
    @builtin(vertex_index) vi: u32,
    @location(0) mn: vec2<f32>,
    @location(1) mx: vec2<f32>,
    @location(2) min_px: vec2<f32>,
    @location(3) color: u32,
) -> VOut {
    let a = to_px(mn);
    let b = to_px(mx);
    let sx = snap_span(a.x, b.x, min_px.x);
    let sy = snap_span(a.y, b.y, min_px.y);
    let k = corner(vi);
    let px = vec2(mix(sx.x, sx.y, k.x), mix(sy.x, sy.y, k.y));
    var o: VOut;
    o.pos = px_to_clip(px);
    o.color = unpack(color);
    o.uv = vec2(0.0);
    o.radius_px = 0.0;
    return o;
}

@vertex
fn vs_segment(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) color: u32,
    @location(3) width_px: f32,
) -> VOut {
    let pa = to_px(a);
    let pb = to_px(b);
    var d = pb - pa;
    let len = length(d);
    d = select(vec2(1.0, 0.0), d / len, len > 1e-6);
    let n = vec2(-d.y, d.x);
    let k = corner(vi);
    let hw = max(width_px, 1.0) * 0.5;
    // extend opaque segments by half-width for clean joins; translucent ones
    // are not extended, so joints are not blended twice
    let ext = select(0.0, hw, (color & 255u) == 255u);
    let along = mix(pa - d * ext, pb + d * ext, k.x);
    let px = along + n * (k.y * 2.0 - 1.0) * hw;
    var o: VOut;
    o.pos = px_to_clip(px);
    o.color = unpack(color);
    o.uv = vec2(0.0);
    o.radius_px = 0.0;
    return o;
}

@vertex
fn vs_point(
    @builtin(vertex_index) vi: u32,
    @location(0) p: vec2<f32>,
    @location(1) color: u32,
    @location(2) radius_px: f32,
) -> VOut {
    let c = to_px(p);
    let k = corner(vi) * 2.0 - 1.0;
    let r = max(radius_px, 0.5) + 1.0; // +1px for anti-aliasing
    var o: VOut;
    o.pos = px_to_clip(c + k * r);
    o.color = unpack(color);
    o.uv = k * r;
    o.radius_px = max(radius_px, 0.5);
    return o;
}

@fragment
fn fs_solid(i: VOut) -> @location(0) vec4<f32> {
    return i.color;
}

@fragment
fn fs_point(i: VOut) -> @location(0) vec4<f32> {
    let a = clamp(i.radius_px - length(i.uv) + 0.5, 0.0, 1.0);
    if (a <= 0.0) {
        discard;
    }
    return vec4(i.color.rgb, i.color.a * a);
}
