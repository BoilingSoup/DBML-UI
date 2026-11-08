//! Mouse injection steps: <x> <y> <down|up|move>
use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (x, y, step): (i16, i16, String) = (
        args[1].parse()?,
        args[2].parse()?,
        args[3].clone(),
    );
    let (conn, _screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[_screen].root;
    if step == "move" {
        conn.xtest_fake_input(6, 0, 0, root, x, y, 0).unwrap();
    } else if step == "down" {
        conn.xtest_fake_input(6, 0, 0, root, x, y, 0).unwrap();
        conn.xtest_fake_input(4, 1, 0, root, 0, 0, 0).unwrap();
    } else if step == "up" {
        // release at the CURRENT pointer position: no move first
        conn.xtest_fake_input(5, 1, 0, root, 0, 0, 0).unwrap();
    }
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(120));
    Ok(())
}
