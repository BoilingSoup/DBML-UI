//! Inject N wheel-up notches at a screen point (zoom-in over the diagram).
//! Args: <x> <y> <notches>
use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (x, y, n): (i16, i16, i32) = (
        args[1].parse()?, args[2].parse()?, args[3].parse()?,
    );
    let (conn, _screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[_screen].root;
    let raw = |ty: u8, detail: u8, x: i16, y: i16| {
        conn.xtest_fake_input(ty, detail, 0, root, x, y, 0).unwrap();
        conn.flush().unwrap();
    };
    raw(6, 0, x, y); // MotionNotify: park the pointer (zoom anchor)
    std::thread::sleep(Duration::from_millis(150));
    // negative notches = wheel down (button 5)
    let (count, btn) = if n < 0 { (-n, 5) } else { (n, 4) };
    for _ in 0..count {
        raw(4, btn, x, y); // ButtonPress
        raw(5, btn, x, y); // ButtonRelease
        std::thread::sleep(Duration::from_millis(60));
    }
    println!("wheel done");
    Ok(())
}
