mod capture;
mod edge_detection;
mod ui;
mod wayland_handlers;

use capture::capture_all_monitors;
use wayland_client::Connection;
use wayland_handlers::WaylandApp;

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");

    // Capture all monitors
    let multi_capture = match capture_all_monitors(&conn) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to capture monitors: {}", e);
            std::process::exit(1);
        }
    };

    let (mut app, mut event_queue) = WaylandApp::new(&conn, multi_capture);
    let qh = event_queue.handle();

    // Roundtrip to ensure outputs are populated before creating surfaces
    event_queue.roundtrip(&mut app).unwrap();

    app.create_surfaces(&qh);

    while !app.should_exit() {
        event_queue.blocking_dispatch(&mut app).unwrap();
    }
}
