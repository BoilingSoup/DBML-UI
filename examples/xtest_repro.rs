//! XTEST key injector: replays position-bug.txt into the focused window
//! on the real X server (so DE modifier grabs apply exactly as they do
//! for a human). Launch the app first, wait for focus, then run this.
//!
//! Sequence: G, k, k, yy, p×5, ctrl+u×3 (each scrolled to completion),
//! e×3, ctrl+u — the repro's step 8. Between chords a bare Control_L
//! release is injected as a best-effort resync (the DE grab may eat it —
//! that IS the environment).

use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

const KEYPRESS: u8 = 2;
const KEYRELEASE: u8 = 3;
// Standard evdev X keycodes (X keycode = evdev scan code + 8).
const CTRL_L: u8 = 37;
const SHIFT_L: u8 = 50;
const KEY_G: u8 = 42;
const KEY_K: u8 = 45;
const KEY_Y: u8 = 29;
const KEY_P: u8 = 33;
const KEY_U: u8 = 30;
const KEY_E: u8 = 26;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, _screen) = x11rb::connect(None)?;
    let raw = |conn: &x11rb::rust_connection::RustConnection, ty: u8, key: u8| {
        conn.xtest_fake_input(ty, key, 0, 0, 0, 0, 0).unwrap();
        conn.flush().unwrap();
    };
    let tap = |conn: &x11rb::rust_connection::RustConnection, key: u8| {
        raw(conn, KEYPRESS, key);
        std::thread::sleep(Duration::from_millis(40));
        raw(conn, KEYRELEASE, key);
    };
    let chord = |conn: &x11rb::rust_connection::RustConnection, key: u8| {
        // best-effort resync: a release the previous chord lost is
        // delivered here (the grab may eat this one too — that IS the
        // environment)
        raw(conn, KEYRELEASE, CTRL_L);
        std::thread::sleep(Duration::from_millis(80));
        raw(conn, KEYPRESS, CTRL_L);
        std::thread::sleep(Duration::from_millis(60));
        tap(conn, key);
        std::thread::sleep(Duration::from_millis(60));
        raw(conn, KEYRELEASE, CTRL_L);
    };
    let say = |s: &str| {
        println!("[{s}]");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    };

    std::thread::sleep(Duration::from_millis(800));
    say("G");
    raw(&conn, KEYPRESS, SHIFT_L);
    std::thread::sleep(Duration::from_millis(50));
    tap(&conn, KEY_G);
    std::thread::sleep(Duration::from_millis(50));
    raw(&conn, KEYRELEASE, SHIFT_L);
    std::thread::sleep(Duration::from_millis(400));
    say("k");
    tap(&conn, KEY_K);
    std::thread::sleep(Duration::from_millis(150));
    say("k");
    tap(&conn, KEY_K);
    std::thread::sleep(Duration::from_millis(150));
    say("yy");
    tap(&conn, KEY_Y);
    tap(&conn, KEY_Y);
    std::thread::sleep(Duration::from_millis(150));
    for _ in 1..=5 {
        say("p");
        tap(&conn, KEY_P);
        std::thread::sleep(Duration::from_millis(150));
    }
    for _ in 1..=3 {
        say("ctrl+u (waiting out the animation)");
        chord(&conn, KEY_U);
        // the neoscroll anim runs 250ms; wait it out like a human
        std::thread::sleep(Duration::from_millis(1000));
    }
    for _ in 1..=3 {
        say("e");
        tap(&conn, KEY_E);
        std::thread::sleep(Duration::from_millis(150));
    }
    say("ctrl+u (STEP 8 — waiting out the animation)");
    chord(&conn, KEY_U);
    std::thread::sleep(Duration::from_millis(1000));
    say("done");
    Ok(())
}
