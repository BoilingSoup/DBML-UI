//! Inject a left-button press+release (XTEST FakeInput) at the current
//! pointer position — move first with xtest_move. Args: none.
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[0].root;
    let (press, release) = (2u8, 3u8);
    conn.xtest_fake_input(press, 1, 0, root, 0, 0, 0).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(60));
    conn.xtest_fake_input(release, 1, 0, root, 0, 0, 0).unwrap();
    conn.flush().unwrap();
    Ok(())
}
