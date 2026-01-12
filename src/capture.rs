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

#[derive(Deserialize)]
struct HyprMonitor {
    name: String,
    focused: bool,
}

/// Get the name of the focused monitor from Hyprland
fn get_focused_monitor_name() -> Option<String> {
    let output = Command::new("hyprctl").args(["monitors", "-j"]).output().ok()?;
    let monitors: Vec<HyprMonitor> = serde_json::from_slice(&output.stdout).ok()?;
    monitors.into_iter().find(|m| m.focused).map(|m| m.name)
}

/// Get the name of the focused output (or None to use first available)
pub fn get_target_output_name() -> Option<String> {
    get_focused_monitor_name()
}

/// Find an output by name, or return the first available
fn find_output_by_name(conn: &Connection, target_name: Option<&str>) -> Result<wl_output::WlOutput, String> {
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
        let _: wl_output::WlOutput = globals
            .registry()
            .bind(global.name, global.version.min(4), &qh, idx);
    }

    // Wait for all outputs to report their info
    while !state.outputs.iter().all(|o| o.done) {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("Dispatch error: {}", e))?;
    }

    // Find by name, or fall back to first
    let mut outputs = state.outputs.into_iter();
    let output = if let Some(name) = target_name {
        outputs.find(|o| o.name.as_deref() == Some(name))
    } else {
        None
    }
    .or_else(|| outputs.next())
    .and_then(|o| o.output);

    output.ok_or_else(|| "No output found".to_string())
}

pub fn capture_screen(conn: &Connection, target_name: Option<&str>) -> Result<Screenshot, String> {
    // First, find the target output
    let output = find_output_by_name(conn, target_name)?;

    let (globals, mut event_queue) = registry_queue_init::<CaptureState>(conn)
        .map_err(|e| format!("Failed to init registry: {}", e))?;

    let qh = event_queue.handle();
    let mut state = CaptureState::new();

    let screencopy_manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 3..=3, ())
        .map_err(|_| "wlr-screencopy protocol not available. Is your compositor wlroots-based?")?;

    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "wl_shm not available")?;

    let frame = screencopy_manager.capture_output(0, &output, &qh, ());

    while !state.done {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("Dispatch error: {}", e))?;
    }

    let format = state.format.ok_or("No suitable buffer format received")?;

    let fd = create_shm_fd().map_err(|e| format!("Failed to create shm fd: {}", e))?;
    let file = File::from(fd);
    let size = (format.stride * format.height) as u64;
    file.set_len(size)
        .map_err(|e| format!("Failed to set file size: {}", e))?;

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
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("Dispatch error: {}", e))?;
    }

    if state.failed {
        return Err("Screen capture failed".to_string());
    }

    let mmap = unsafe { MmapMut::map_mut(&file) }.map_err(|e| format!("Failed to mmap: {}", e))?;
    let data = mmap.to_vec();

    // Pre-compute luminance and convert to BGRA in one pass
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

                luminance[dst_idx] = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;

                let bgra_idx = dst_idx * 4;
                bgra_data[bgra_idx] = b;
                bgra_data[bgra_idx + 1] = g;
                bgra_data[bgra_idx + 2] = r;
                bgra_data[bgra_idx + 3] = 255;
            }
        }
    }

    buffer.destroy();
    shm_pool.destroy();
    frame.destroy();

    Ok(Screenshot {
        bgra_data,
        width: format.width,
        height: format.height,
        luminance,
    })
}
