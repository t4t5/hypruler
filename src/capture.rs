use memmap2::MmapMut;
use rustix::fs::{self, SealFlags};
use smithay_client_toolkit::output::OutputState;
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
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

#[derive(Debug)]
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
#[derive(Debug)]
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
#[derive(Debug)]
pub struct MultiMonitorCapture {
    pub monitors: Vec<MonitorInfo>,
}

impl MultiMonitorCapture {
    // Methods can be added here as needed
}

/// Output geometry and properties reported by xdg-output.
#[derive(Debug, Clone)]
pub struct OutputMetadata {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub transform: u32,
}

fn output_is_rotated(transform: u32) -> bool {
    matches!(transform, 1 | 3 | 5 | 7)
}

fn output_scale_hint(
    info: &smithay_client_toolkit::output::OutputInfo,
    width: u32,
    height: u32,
    transform: u32,
) -> f64 {
    if let Some(mode) = info.modes.iter().find(|mode| mode.current) {
        let (physical_width, physical_height) = mode.dimensions;
        if physical_width > 0 && physical_height > 0 {
            let (physical_width, physical_height) = if output_is_rotated(transform) {
                (physical_height, physical_width)
            } else {
                (physical_width, physical_height)
            };
            let scale_x = physical_width as f64 / width as f64;
            let scale_y = physical_height as f64 / height as f64;
            if scale_x > 0.0 && (scale_x - scale_y).abs() < 0.01 {
                return (scale_x + scale_y) / 2.0;
            }
        }
    }

    info.scale_factor.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::{apply_transform, output_is_rotated};

    #[test]
    fn recognizes_rotated_output_transforms() {
        assert!(output_is_rotated(1));
        assert!(output_is_rotated(3));
        assert!(output_is_rotated(5));
        assert!(output_is_rotated(7));
        assert!(!output_is_rotated(0));
        assert!(!output_is_rotated(2));
        assert!(!output_is_rotated(4));
        assert!(!output_is_rotated(6));
    }

    #[test]
    fn transforms_flipped_rotations() {
        for transform in [5, 7] {
            let (width, height, luminance, bgra) =
                apply_transform(2, 3, vec![0; 6], vec![0; 24], transform);
            assert_eq!((width, height), (3, 2));
            assert_eq!(luminance.len(), 6);
            assert_eq!(bgra.len(), 24);
        }
    }
}

/// Read enabled output geometry from SCTK's xdg-output-backed `OutputState`.
fn discover_outputs(
    output_state: &OutputState,
) -> Result<Vec<(OutputMetadata, wl_output::WlOutput)>, String> {
    let mut discovered = Vec::new();

    for (index, output) in output_state.outputs().enumerate() {
        let info = output_state
            .info(&output)
            .ok_or("Output information is not available yet")?;
        let name = info
            .name
            .clone()
            .unwrap_or_else(|| format!("output-{index}"));
        let (x, y) = info
            .logical_position
            .ok_or_else(|| format!("Output {name} did not provide a logical position"))?;
        let (width, height) = info
            .logical_size
            .ok_or_else(|| format!("Output {name} did not provide a logical size"))?;
        if width <= 0 || height <= 0 {
            return Err(format!("Output {name} has an invalid logical size"));
        }
        let transform = info.transform as u32;

        discovered.push((
            OutputMetadata {
                name,
                x,
                y,
                width: width as u32,
                height: height as u32,
                scale: output_scale_hint(&info, width as u32, height as u32, transform),
                transform,
            },
            output,
        ));
    }

    if discovered.is_empty() {
        return Err("No outputs were reported by xdg-output".to_string());
    }

    Ok(discovered)
}

/// Capture all monitors and return MultiMonitorCapture.
pub fn capture_all_monitors(
    conn: &Connection,
    output_state: &OutputState,
) -> Result<MultiMonitorCapture, String> {
    let outputs = discover_outputs(output_state)?;

    // Build monitor info structure from compositor-independent output metadata.
    let mut monitors = outputs
        .into_iter()
        .map(|(info, output)| MonitorInfo {
            name: info.name,
            output,
            x: info.x,
            y: info.y,
            width: info.width,
            height: info.height,
            scale: info.scale,
            transform: info.transform,
            screenshot: None,
        })
        .collect::<Vec<_>>();

    if monitors.is_empty() {
        return Err("No monitors found".to_string());
    }

    // Capture each monitor
    let (globals, mut event_queue) = registry_queue_init::<CaptureState>(conn)
        .map_err(|e| format!("Failed to init registry: {}", e))?;

    let qh = event_queue.handle();

    let screencopy_manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 3..=3, ())
        .map_err(|_| "wlr-screencopy-unstable-v1 protocol is not available")?;

    let shm: wl_shm::WlShm = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "wl_shm not available")?;

    // Capture each monitor. Keep running if an individual output fails, but report why.
    for monitor in &mut monitors {
        let capture_result = (|| -> Result<Screenshot, String> {
            let mut state = CaptureState::new();
            let frame = screencopy_manager.capture_output(0, &monitor.output, &qh, ());

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

            let capture_status = (|| -> Result<(), String> {
                while !state.ready && !state.failed {
                    event_queue
                        .blocking_dispatch(&mut state)
                        .map_err(|e| format!("Dispatch error: {}", e))?;
                }

                if state.failed {
                    return Err("Screen capture failed".to_string());
                }

                Ok(())
            })();

            if let Err(e) = capture_status {
                buffer.destroy();
                shm_pool.destroy();
                frame.destroy();
                return Err(e);
            }

            let mmap = match unsafe { MmapMut::map_mut(&file) } {
                Ok(mmap) => mmap,
                Err(e) => {
                    buffer.destroy();
                    shm_pool.destroy();
                    frame.destroy();
                    return Err(format!("Failed to mmap: {}", e));
                }
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

            let (final_width, final_height, final_luminance, final_bgra) = apply_transform(
                format.width,
                format.height,
                luminance,
                bgra_data,
                monitor.transform,
            );

            buffer.destroy();
            shm_pool.destroy();
            frame.destroy();

            Ok(Screenshot {
                bgra_data: final_bgra,
                width: final_width,
                height: final_height,
                luminance: final_luminance,
            })
        })();

        match capture_result {
            Ok(screenshot) => monitor.screenshot = Some(screenshot),
            Err(e) => eprintln!("warning: capture failed for {}: {}", monitor.name, e),
        }
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
    let (new_width, new_height) = match transform {
        1 | 3 | 5 | 7 => (height, width),
        2 | 4 | 6 => (width, height),
        _ => return (width, height, luminance, bgra_data),
    };
    let new_pixel_count = (new_width * new_height) as usize;
    let mut transformed_luminance = vec![0u8; new_pixel_count];
    let mut transformed_bgra = vec![0u8; new_pixel_count * 4];

    for y in 0..height {
        for x in 0..width {
            let (new_x, new_y) = match transform {
                1 => (height - 1 - y, x),
                2 => (width - 1 - x, height - 1 - y),
                3 => (y, width - 1 - x),
                4 => (width - 1 - x, y),
                5 => (height - 1 - y, width - 1 - x),
                6 => (x, height - 1 - y),
                7 => (y, x),
                _ => unreachable!(),
            };

            let src_idx = (y * width + x) as usize;
            let dst_idx = (new_y * new_width + new_x) as usize;
            transformed_luminance[dst_idx] = luminance[src_idx];

            let src_bgra = src_idx * 4;
            let dst_bgra = dst_idx * 4;
            transformed_bgra[dst_bgra..dst_bgra + 4]
                .copy_from_slice(&bgra_data[src_bgra..src_bgra + 4]);
        }
    }

    (
        new_width,
        new_height,
        transformed_luminance,
        transformed_bgra,
    )
}
