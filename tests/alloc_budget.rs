//! Allocation-budget regression test for the per-frame composition path.
//!
//! Runs `compose_frame` in a tight loop and asserts that bytes-allocated and
//! allocation-count per frame stay below generous thresholds. The point isn't
//! proving the path is allocation-free — it's blocking accidental pixel-buffer
//! churn (e.g. a `Screenshot::clone()` slipping into the inner loop).
//!
//! Hardware-independent and noise-free — suitable for CI pass/fail.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use hypruler::capture::Screenshot;
use hypruler::render::{FrameOverlay, compose_frame};
use tiny_skia::Pixmap;

static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Track only growth; shrinks don't allocate fresh capacity.
        if new_size > layout.size() {
            ALLOC_BYTES.fetch_add(new_size - layout.size(), Ordering::Relaxed);
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

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

/// Per-frame allocation budgets. Set well above measured steady-state so normal
/// refactoring has headroom; the goal is to catch order-of-magnitude regressions
/// (e.g. cloning the screenshot, ~41 MB at 4K) rather than enforce purity.
///
/// As of writing, a 4K drag frame measured ~48 KiB / 51 allocs in steady state,
/// so these budgets give roughly 5× headroom on both axes.
const BYTE_BUDGET_PER_FRAME: usize = 256 * 1024; // 256 KiB
const ALLOC_BUDGET_PER_FRAME: usize = 256;

#[test]
fn compose_frame_4k_drag_stays_under_budget() {
    const WIDTH: u32 = 3840;
    const HEIGHT: u32 = 2160;
    const ITERS: usize = 100;

    // --- Setup (allocations here are ignored; we sample the counters after.)
    let screenshot = synthetic_screenshot(WIDTH, HEIGHT);
    let mut canvas = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    let mut cached_pixmap = Some(Pixmap::new(WIDTH, HEIGHT).unwrap());
    let overlay = FrameOverlay {
        pointer_x: WIDTH as f64 * 0.75,
        pointer_y: HEIGHT as f64 * 0.65,
        scale: 1.0,
        drag_start: Some((WIDTH as f64 * 0.25, HEIGHT as f64 * 0.35)),
        drag_rect: None,
        is_dragging: true,
        debug_fps: None,
    };

    // Warm-up: first call lazily allocates path builders, glyph atlases, etc.
    compose_frame(&mut canvas, &mut cached_pixmap, &screenshot, overlay, None).unwrap();

    // --- Measure steady state.
    let bytes_before = ALLOC_BYTES.load(Ordering::Relaxed);
    let count_before = ALLOC_COUNT.load(Ordering::Relaxed);

    for _ in 0..ITERS {
        compose_frame(&mut canvas, &mut cached_pixmap, &screenshot, overlay, None).unwrap();
    }

    let bytes_total = ALLOC_BYTES.load(Ordering::Relaxed) - bytes_before;
    let count_total = ALLOC_COUNT.load(Ordering::Relaxed) - count_before;
    let bytes_per_frame = bytes_total / ITERS;
    let allocs_per_frame = count_total / ITERS;

    // Always print so CI logs show the trend even on success.
    println!(
        "compose_frame 4K drag: {bytes_per_frame} bytes/frame, {allocs_per_frame} allocs/frame \
         (budget: {BYTE_BUDGET_PER_FRAME} bytes, {ALLOC_BUDGET_PER_FRAME} allocs)"
    );

    assert!(
        bytes_per_frame < BYTE_BUDGET_PER_FRAME,
        "bytes/frame regression: {bytes_per_frame} >= budget {BYTE_BUDGET_PER_FRAME}. \
         A pixel-buffer clone may have been introduced into compose_frame."
    );
    assert!(
        allocs_per_frame < ALLOC_BUDGET_PER_FRAME,
        "allocs/frame regression: {allocs_per_frame} >= budget {ALLOC_BUDGET_PER_FRAME}. \
         A new per-frame allocation pattern may have been introduced."
    );
}
