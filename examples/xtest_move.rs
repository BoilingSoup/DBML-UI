//! Move the pointer to a screen position (hover injection). Args: <x> <y>
use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (x, y): (i16, i16) = (args[1].parse()?, args[2].parse()?);
    let (conn, _screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[_screen].root;
    conn.xtest_fake_input(6, 0, 0, root, x, y, 0).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}
