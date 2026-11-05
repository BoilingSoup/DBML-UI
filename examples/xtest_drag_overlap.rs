//! Drag a table over another on a GIVEN display (XDG/Xvfb) and
//! screenshot the canvas, to inspect drag-priority rendering.
//!
//! Args: <display> <out_png> <drag_x> <drag_y> <drop_x> <drop_y>
//! Coordinates are SCREEN px of the press point and drop point.

use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (_, dx, dy, ox, oy) = (
        &args[1],
        args[2].parse::<i16>()?,
        args[3].parse::<i16>()?,
        args[4].parse::<i16>()?,
        args[5].parse::<i16>()?,
    );
    let _ = &args;
    let (conn, _screen) = x11rb::connect(None)?;
    // XTEST events go to the ROOT window with screen-relative coordinates
    let root = conn.setup().roots[_screen].root;
    let rootw = root;
    let motion = |conn: &x11rb::rust_connection::RustConnection, rootw: x11rb::protocol::xproto::Window, x: i16, y: i16| {
        conn.xtest_fake_input(6, 0, 0, rootw, x, y, 0).unwrap();
        conn.flush().unwrap();
    };
    let button = |conn: &x11rb::rust_connection::RustConnection, rootw: x11rb::protocol::xproto::Window, ty: u8, x: i16, y: i16| {
        conn.xtest_fake_input(ty, 1, 0, rootw, x, y, 0).unwrap();
        conn.flush().unwrap();
    };
    // park, press, drag in small steps (each step a MotionNotify event),
    // release
    motion(&conn, rootw, dx, dy);
    std::thread::sleep(Duration::from_millis(150));
    button(&conn, rootw, 4, dx, dy); // ButtonPress
    std::thread::sleep(Duration::from_millis(120));
    let steps = 24;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        motion(
            &conn,
            rootw,
            dx + ((ox - dx) as f32 * t) as i16,
            dy + ((oy - dy) as f32 * t) as i16,
        );
        std::thread::sleep(Duration::from_millis(30));
    }
    std::thread::sleep(Duration::from_millis(200));
    button(&conn, rootw, 5, ox, oy); // ButtonRelease
    std::thread::sleep(Duration::from_millis(300));
    println!("drag done");
    Ok(())
}
