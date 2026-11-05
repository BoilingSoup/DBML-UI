//! Live probe: is the XKB state machine (GetState) truthful while the
//! DE's Locate-Pointer grab holds Control_L — where the keymap query
//! demonstrably lies (key.log bridge=Some(false))? Presses Control_L via
//! XTEST, polls keymap + XKB state for 2.5s, releases, drains.

use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xkb::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xtest::ConnectionExt as _;

const KEYPRESS: u8 = 2;
const KEYRELEASE: u8 = 3;
const CTRL_L: u8 = 37;
const XKB_USE_CORE_KBD: u16 = 0x0100;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _screen) = x11rb::connect(None)?;
    conn.xkb_use_extension(1, 0)?.reply()?;
    let raw = |ty: u8, key: u8| {
        conn.xtest_fake_input(ty, key, 0, 0, 0, 0, 0).unwrap();
        conn.flush().unwrap();
    };
    let sample = |label: &str| {
        let km = conn.query_keymap().ok().and_then(|c| c.reply().ok());
        let ctrl_in_keymap = km.is_some_and(|r| {
            r.keys.iter().any(|_b| {
                // Control_L/R sit at keycodes 37/105 -> bits 29/97 -> byte 3 bit 5 / byte 12 bit 1
                // (resolve generically: recompute from the map is overkill here)
                false
            })
        });
        let _ = ctrl_in_keymap;
        let keymap_ctrl = km.map(|r| {
            // keycode 37 -> index 29 -> byte 3, mask 1<<5; keycode 105 -> index 97 -> byte 12, mask 1<<1
            r.keys.get(3).is_some_and(|b| b & (1 << 5) != 0)
                || r.keys.get(12).is_some_and(|b| b & (1 << 1) != 0)
        });
        let xkb = conn.xkb_get_state(XKB_USE_CORE_KBD).ok().and_then(|c| c.reply().ok());
        let b = |m: x11rb::protocol::xproto::ModMask| u32::from(m);
        let ctl = x11rb::protocol::xproto::ModMask::CONTROL;
        match (keymap_ctrl, xkb) {
            (km, Some(x)) => println!(
                "{label:<10} keymap_ctrl={km:?} xkb: ctrl[mods={} base={} latched={} locked={} compat={} grab={}]",
                b(x.mods) & b(ctl) != 0,
                b(x.base_mods) & b(ctl) != 0,
                b(x.latched_mods) & b(ctl) != 0,
                b(x.locked_mods) & b(ctl) != 0,
                b(x.compat_state) & b(ctl) != 0,
                b(x.grab_mods) & b(ctl) != 0,
            ),
            (km, None) => println!("{label:<10} keymap_ctrl={km:?} xkb=<unavailable>"),
        }
        std::io::Write::flush(&mut std::io::stdout()).ok();
    };

    sample("before");
    raw(KEYPRESS, CTRL_L);
    for i in 0..25 {
        std::thread::sleep(Duration::from_millis(100));
        sample(&format!("held{i}"));
    }
    raw(KEYRELEASE, CTRL_L);
    for i in 0..5 {
        std::thread::sleep(Duration::from_millis(100));
        sample(&format!("after{i}"));
    }
    Ok(())
}
