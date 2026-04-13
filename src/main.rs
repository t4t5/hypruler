mod capture;
mod edge_detection;
mod ui;
mod wayland_handlers;

use capture::{capture_screen, get_focused_monitor_info};
use wayland_client::Connection;
use wayland_handlers::WaylandApp;

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");

    let monitor_info = get_focused_monitor_info();
    let target_output_name = monitor_info.as_ref().map(|(name, _)| name.clone());
    let transform = monitor_info.map(|(_, t)| t).unwrap_or(0);

    let screenshot = match capture_screen(&conn, target_output_name.as_deref(), transform) {
        Ok(s) => s,
        Err(_) => std::process::exit(1),
    };

    let (mut app, mut event_queue) = WaylandApp::new(&conn, screenshot, target_output_name);
    let qh = event_queue.handle();

    // Roundtrip to ensure outputs are populated before creating surface
    event_queue.roundtrip(&mut app).unwrap();

    app.create_surface(&qh);

    while !app.should_exit() {
        event_queue.blocking_dispatch(&mut app).unwrap();
    }
}
