use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use hypruler::capture::Screenshot;
use hypruler::render::{FrameOverlay, compose_frame};
use tiny_skia::Pixmap;

fn synthetic_screenshot(width: u32, height: u32) -> Screenshot {
    let mut bgra_data = Vec::with_capacity(width as usize * height as usize * 4);

    for y in 0..height {
        for x in 0..width {
            let gradient = ((x ^ y) & 0xff) as u8;
            bgra_data.push(gradient);
            bgra_data.push(y as u8);
            bgra_data.push(x as u8);
            bgra_data.push(255);
        }
    }

    Screenshot::from_bgra_data(width, height, bgra_data).unwrap()
}

fn bench_drag_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("compose_drag_frame");

    for (width, height) in [(1920, 1080), (3840, 2160)] {
        let screenshot = synthetic_screenshot(width, height);
        let overlay = FrameOverlay {
            pointer_x: width as f64 * 0.75,
            pointer_y: height as f64 * 0.65,
            scale: 1.0,
            drag_start: Some((width as f64 * 0.25, height as f64 * 0.35)),
            drag_rect: None,
            is_dragging: true,
            debug_fps: None,
        };

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{width}x{height}")),
            &(width, height),
            |b, &(width, height)| {
                let mut canvas = vec![0u8; width as usize * height as usize * 4];
                let mut cached_pixmap = Some(Pixmap::new(width, height).unwrap());

                b.iter(|| {
                    compose_frame(&mut canvas, &mut cached_pixmap, &screenshot, overlay, None)
                        .unwrap();
                    criterion::black_box(&canvas);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_drag_frame);
criterion_main!(benches);
