//! Headless render: candles + SMA + scatter into a texture, saved as PPM.
//! `cargo run -p photic-gpu --example offscreen -- out.ppm`
use photic_core::{lod_level_for, Camera2D, Point, Rgba};
use photic_finance::{candles, line, random_walk, sma, CandleStyle, LineSeries, OhlcSeries};
use photic_gpu::{wgpu, Layer, Renderer};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "offscreen.ppm".into());
    let (w, h) = (1200u32, 600u32);
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("device");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let renderer = Renderer::new(&device, format, 1);

    // 1M bars, view the last 300 at full detail.
    let series = OhlcSeries::new(random_walk(1_000_000, 42));
    let n = series.len();
    let range = (n - 300, n);
    let (lo, hi) = series.price_range(range.0, range.1).unwrap();
    let pad = (hi - lo) * 0.05;
    let cam = Camera2D::new((range.0 as f64, range.1 as f64), (lo - pad, hi + pad), (w as f64, h as f64));
    let origin = (range.0 as f64, lo);
    let level = lod_level_for((range.1 - range.0) as f64 / w as f64, 0.25, series.pyramid.max_level());

    let mut rects = Vec::new();
    candles(&series, level, range, origin, &CandleStyle::default(), &mut rects);
    let mut segs = Vec::new();
    let ma = sma(series.bars(), 20);
    line(&LineSeries::new(&ma), 0, range, origin, Rgba::rgb(255, 193, 7), 2.0, &mut segs);
    let pts: Vec<Point> = (range.0..range.1).step_by(25)
        .map(|i| Point { pos: [(i - range.0) as f32 + 0.5, (series.bars()[i].high - lo + pad * 0.5) as f32], color: Rgba::rgba(66, 165, 245, 200), radius_px: 4.0 })
        .collect();

    let mut layer = Layer::new(&device, &renderer);
    layer.set_rects(&device, &queue, &rects);
    layer.set_segments(&device, &queue, &segs);
    layer.set_points(&device, &queue, &pts);
    layer.set_view(&queue, cam.gpu_transform(origin), [w as f32, h as f32]);

    let view = tex.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.07, g: 0.08, b: 0.1, a: 1.0 }), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        renderer.draw(&mut pass, &layer);
    }
    let bpr = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (bpr * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: None } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buf.slice(..).get_mapped_range();
    let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
    for y in 0..h {
        for x in 0..w {
            let i = (y * bpr + x * 4) as usize;
            ppm.extend_from_slice(&data[i..i + 3]);
        }
    }
    std::fs::write(&out, ppm).unwrap();
    println!("wrote {out}: {} rects, {} segments, {} points, LOD level {level}", rects.len(), segs.len(), pts.len());
}
