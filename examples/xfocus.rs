//! Set X input focus to a window (the rig has no WM). Args: <window id>
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let win = u32::from_str_radix(args[1].trim_start_matches("0x"), 16)?;
    let (conn, _screen) = x11rb::connect(None)?;
    conn.set_input_focus(
        x11rb::protocol::xproto::InputFocus::POINTER_ROOT,
        win,
        x11rb::protocol::xproto::Time::CURRENT_TIME,
    )?;
    conn.flush().unwrap();
    Ok(())
}
