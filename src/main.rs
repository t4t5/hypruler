mod capture;
mod edge_detection;
mod ui;
mod wayland_handlers;

use capture::{capture_screen, get_target_output_name};
use wayland_client::Connection;
use wayland_handlers::WaylandApp;

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");

    let target_output_name = get_target_output_name();

    let screenshot = match capture_screen(&conn, target_output_name.as_deref()) {
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
