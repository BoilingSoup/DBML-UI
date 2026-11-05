//! Set X input focus to a window (the rig has no WM). Args: <window id>
//!
//! SetInputFocus alone leaves winit unconvinced — it tracks focus via
//! FocusIn/FocusOut events, and with no WM none ever arrive, so iced's
//! keyboard stays gated off. A synthetic FocusIn sent straight to the
//! window sells it.
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xproto::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let win = u32::from_str_radix(args[1].trim_start_matches("0x"), 16)?;
    let (conn, _screen) = x11rb::connect(None)?;
    conn.set_input_focus(
        InputFocus::PARENT,
        win,
        Time::CURRENT_TIME,
    )?;

    // a FocusIn event, event-win = the window itself: winit marks the
    // window focused and iced's keyboard gates open
    let mut buf = [0u8; 32];
    buf[0] = 9; // FocusIn
    buf[1] = u8::from(NotifyDetail::POINTER); // detail
    buf[2] = u8::from(NotifyMode::NORMAL); // mode
    buf[4..8].copy_from_slice(&win.to_ne_bytes()); // event
    buf[8..12].copy_from_slice(&win.to_ne_bytes()); // window
    conn.send_event(false, win, EventMask::NO_EVENT, buf as [u8; 32])?;
    conn.flush()?;
    Ok(())
}
