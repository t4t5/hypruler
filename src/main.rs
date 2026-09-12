mod capture;
mod edge_detection;
mod fps;
mod ui;
mod wayland_handlers;

use capture::capture_all_monitors;
use wayland_client::Connection;
use wayland_handlers::WaylandApp;

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to connect to Wayland");

    let (mut app, mut event_queue) = WaylandApp::new(&conn);

    // Populate SCTK's output state, including xdg-output logical geometry.
    event_queue.roundtrip(&mut app).unwrap();

    let multi_capture = match capture_all_monitors(&conn, app.output_state()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to capture monitors: {}", e);
            std::process::exit(1);
        }
    };
    app.set_capture(multi_capture);

    let qh = event_queue.handle();

    if let Err(e) = app.create_surfaces(&qh) {
        eprintln!("Failed to create monitor surfaces: {}", e);
        std::process::exit(1);
    }

    while !app.should_exit() {
        event_queue.blocking_dispatch(&mut app).unwrap();
    }
}
