use memmap2::MmapMut;
use rustix::fs::{self, SealFlags};
use serde::Deserialize;
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::process::Command;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

#[derive(Debug, Clone, Copy)]
struct FrameFormat {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Debug, Clone, Default)]
pub struct OutputInfo {
    pub name: Option<String>,
    pub output: Option<wl_output::WlOutput>,
    done: bool,
}

struct OutputEnumState {
    outputs: Vec<OutputInfo>,
}

struct CaptureState {
    format: Option<FrameFormat>,
    done: bool,
    ready: bool,
    failed: bool,
}

impl CaptureState {
    fn new() -> Self {
        Self {
            format: None,
            done: false,
            ready: false,
            failed: false,
        }
    }
}

// Dispatch implementations for output enumeration
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for OutputEnumState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, usize> for OutputEnumState {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        data: &usize,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let idx = *data;
        if idx >= state.outputs.len() {
            return;
        }
        let info = &mut state.outputs[idx];

        match event {
            wl_output::Event::Name { name } => {
                info.name = Some(name);
                info.output = Some(proxy.clone());
            }
            wl_output::Event::Done => {
                info.done = true;
            }
            _ => {}
        }
    }
}

// Dispatch implementations for screen capture
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrScreencopyManagerV1,
        _event: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: wayland_client::WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                state.format = Some(FrameFormat {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                state.done = true;
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                state.ready = true;
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.failed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm::WlShm,
        _event: wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_shm_pool::WlShmPool,
        _event: <wl_shm_pool::WlShmPool as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_buffer::WlBuffer,
        _event: wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for CaptureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_output::WlOutput,
        _event: wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

fn create_shm_fd() -> std::io::Result<OwnedFd> {
    loop {
        match fs::memfd_create(
            CString::new("hypruler-capture")?.as_c_str(),
            fs::MemfdFlags::CLOEXEC | fs::MemfdFlags::ALLOW_SEALING,
        ) {
            Ok(fd) => {
                let _ = fs::fcntl_add_seals(&fd, SealFlags::SHRINK | SealFlags::SEAL);
                return Ok(fd);
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(errno) => return Err(std::io::Error::from(errno)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Screenshot {
    bgra_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    luminance: Vec<u8>,
}

impl Screenshot {
    pub fn bgra_data(&self) -> &[u8] {
        &self.bgra_data
    }

    pub fn get_luminance(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.luminance[(y * self.width + x) as usize]
    }
}

/// Information about a monitor including its screenshot
#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub name: String,
    pub output: wl_output::WlOutput,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub transform: u32,
    pub screenshot: Option<Screenshot>,
}

/// All monitors with their screenshots
#[derive(Debug, Clone)]
pub struct MultiMonitorCapture {
    pub monitors: Vec<MonitorInfo>,
}

impl MultiMonitorCapture {
    // Methods can be added here as needed
}

/// Get all monitor info from Hyprland
#[derive(Deserialize)]
struct HyprMonitorFull {
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    transform: Option<u32>,
}

pub fn get_all_monitor_info() -> Option<Vec<(String, i32, i32, u32, u32, f64, u32)>> {
    let output = Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .ok()?;
    let monitors: Vec<HyprMonitorFull> = serde_json::from_slice(&output.stdout).ok()?;
    Some(
        monitors
            .into_iter()
            .map(|m| {
                (
                    m.name,
                    m.x,
                    m.y,
                    m.width,
                    m.height,
                    m.scale,
                    m.transform.unwrap_or(0),
                )
            })
            .collect(),
    )
}

/// Find all outputs and return their info
fn find_all_outputs(conn: &Connection) -> Result<Vec<(String, wl_output::WlOutput)>, String> {
    let (globals, mut event_queue) = registry_queue_init::<OutputEnumState>(conn)
        .map_err(|e| format!("Failed to init registry: {}", e))?;

    let qh = event_queue.handle();
    let output_globals: Vec<_> = globals
        .contents()
        .clone_list()
        .into_iter()
        .filter(|g| g.interface == "wl_output")
        .collect();

    if output_globals.is_empty() {
        return Err("No outputs available".to_string());
    }

    let mut state = OutputEnumState {
        outputs: vec![OutputInfo::default(); output_globals.len()],
    };

    // Bind all outputs
    for (idx, global) in output_globals.iter().enumerate() {
        let output: wl_output::WlOutput =
            globals
                .registry()
                .bind(global.name, global.version.min(4), &qh, idx);
        state.outputs[idx].output = Some(output);
    }

    // Wait for all outputs to report their info
    while !state.outputs.iter().all(|o| o.done) {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("Dispatch error: {}", e))?;
    }

    Ok(state
        .outputs
        .into_iter()
        .filter_map(|o| o.name.zip(o.output))
        .collect())
}

/// Capture all monitors and return MultiMonitorCapture
pub fn capture_all_monitors(conn: &Connection) -> Result<MultiMonitorCapture, String> {
    // Get monitor info from Hyprland
    let hypr_info = get_all_monitor_info().ok_or("Failed to get monitor info from Hyprland")?;

    // Find all Wayland outputs
    let outputs = find_all_outputs(conn)?;

    // Build monitor info structure
    let mut monitors = Vec::new();
    for (name, x, y, width, height, scale, transform) in hypr_info {
        // Find the corresponding Wayland output
        if let Some((_, output)) = outputs.iter().find(|(n, _)| n == &name).cloned() {
            monitors.push(MonitorInfo {
                name,
                output,
                x,
                y,
                width,
                height,
                scale,
                transform,
                screenshot: None,
            });
        }
    }

    if monitors.is_empty() {
        return Err("No monitors found".to_string());
    }

    // Capture each monitor
    let (globals, mut event_queue) = registry_queue_init::<CaptureState>(conn)
        .map_err(|e| format!("Failed to init registry: {}", e))?;

    let qh = event_queue.handle();

    let screencopy_manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 3..=3, ())
        .map_err(|_| "wlr-screencopy protocol not available. Is your compositor wlroots-based?")?;

    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "wl_shm not available")?;

    // Capture each monitor
    for monitor in &mut monitors {
        let mut state = CaptureState::new();
        let frame = screencopy_manager.capture_output(0, &monitor.output, &qh, ());

        while !state.done {
            event_queue
                .blocking_dispatch(&mut state)
                .map_err(|e| format!("Dispatch error: {}", e))?;
        }

        let format = match state.format {
            Some(f) => f,
            None => continue, // Skip this monitor if format not available
        };

        let fd = create_shm_fd().map_err(|e| format!("Failed to create shm fd: {}", e))?;
        let file = File::from(fd);
        let size = (format.stride * format.height) as u64;
        if file.set_len(size).is_err() {
            continue;
        }

        let shm_pool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer = shm_pool.create_buffer(
            0,
            format.width as i32,
            format.height as i32,
            format.stride as i32,
            format.format,
            &qh,
            (),
        );

        frame.copy(&buffer);

        while !state.ready && !state.failed {
            if event_queue.blocking_dispatch(&mut state).is_err() {
                break;
            }
        }

        if state.failed {
            buffer.destroy();
            shm_pool.destroy();
            frame.destroy();
            continue;
        }

        let Ok(mmap) = (unsafe { MmapMut::map_mut(&file) }) else {
            buffer.destroy();
            shm_pool.destroy();
            frame.destroy();
            continue;
        };
        let data = mmap.to_vec();

        // Pre-compute luminance and convert to BGRA
        let pixel_count = (format.width * format.height) as usize;
        let mut luminance = vec![0u8; pixel_count];
        let mut bgra_data = vec![0u8; pixel_count * 4];

        for y in 0..format.height {
            for x in 0..format.width {
                let src_idx = (y * format.stride + x * 4) as usize;
                let dst_idx = (y * format.width + x) as usize;

                if src_idx + 3 < data.len() {
                    let (r, g, b) = match format.format {
                        wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => {
                            (data[src_idx + 2], data[src_idx + 1], data[src_idx])
                        }
                        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => {
                            (data[src_idx], data[src_idx + 1], data[src_idx + 2])
                        }
                        _ => (data[src_idx + 2], data[src_idx + 1], data[src_idx]),
                    };

                    luminance[dst_idx] =
                        (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;

                    let bgra_idx = dst_idx * 4;
                    bgra_data[bgra_idx] = b;
                    bgra_data[bgra_idx + 1] = g;
                    bgra_data[bgra_idx + 2] = r;
                    bgra_data[bgra_idx + 3] = 255;
                }
            }
        }

        // Apply transform
        let (final_width, final_height, final_luminance, final_bgra) = apply_transform(
            format.width,
            format.height,
            luminance,
            bgra_data,
            monitor.transform,
        );

        monitor.screenshot = Some(Screenshot {
            bgra_data: final_bgra,
            width: final_width,
            height: final_height,
            luminance: final_luminance,
        });

        buffer.destroy();
        shm_pool.destroy();
        frame.destroy();
    }

    if monitors.iter().all(|m| m.screenshot.is_none()) {
        return Err("Failed to capture any monitor".to_string());
    }

    Ok(MultiMonitorCapture { monitors })
}

fn apply_transform(
    width: u32,
    height: u32,
    luminance: Vec<u8>,
    bgra_data: Vec<u8>,
    transform: u32,
) -> (u32, u32, Vec<u8>, Vec<u8>) {
    match transform {
        1 | 3 => {
            let new_width = height;
            let new_height = width;
            let new_pixel_count = (new_width * new_height) as usize;
            let mut rotated_luminance = vec![0u8; new_pixel_count];
            let mut rotated_bgra = vec![0u8; new_pixel_count * 4];

            for y in 0..height {
                for x in 0..width {
                    let (new_x, new_y) = if transform == 1 {
                        (height - 1 - y, x)
                    } else {
                        (y, width - 1 - x)
                    };

                    let src_idx = (y * width + x) as usize;
                    let dst_idx = (new_y * new_width + new_x) as usize;

                    rotated_luminance[dst_idx] = luminance[src_idx];

                    let src_bgra = src_idx * 4;
                    let dst_bgra = dst_idx * 4;
                    rotated_bgra[dst_bgra] = bgra_data[src_bgra];
                    rotated_bgra[dst_bgra + 1] = bgra_data[src_bgra + 1];
                    rotated_bgra[dst_bgra + 2] = bgra_data[src_bgra + 2];
                    rotated_bgra[dst_bgra + 3] = bgra_data[src_bgra + 3];
                }
            }

            (new_width, new_height, rotated_luminance, rotated_bgra)
        }
        2 => {
            let pixel_count = (width * height) as usize;
            let mut rotated_luminance = vec![0u8; pixel_count];
            let mut rotated_bgra = vec![0u8; pixel_count * 4];

            for y in 0..height {
                for x in 0..width {
                    let new_x = width - 1 - x;
                    let new_y = height - 1 - y;

                    let src_idx = (y * width + x) as usize;
                    let dst_idx = (new_y * width + new_x) as usize;

                    rotated_luminance[dst_idx] = luminance[src_idx];

                    let src_bgra = src_idx * 4;
                    let dst_bgra = dst_idx * 4;
                    rotated_bgra[dst_bgra] = bgra_data[src_bgra];
                    rotated_bgra[dst_bgra + 1] = bgra_data[src_bgra + 1];
                    rotated_bgra[dst_bgra + 2] = bgra_data[src_bgra + 2];
                    rotated_bgra[dst_bgra + 3] = bgra_data[src_bgra + 3];
                }
            }

            (width, height, rotated_luminance, rotated_bgra)
        }
        _ => (width, height, luminance, bgra_data),
    }
}
