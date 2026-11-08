//! Does the app's own window end up ACTIVE after launch (raise_self)?
//! Reads _NET_ACTIVE_WINDOW before/after spawning the real binary.
use std::process::Command;
use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    };
    let active = atom(b"_NET_ACTIVE_WINDOW")?;
    let pid_atom = atom(b"_NET_WM_PID")?;
    fn read_active(
        conn: &x11rb::rust_connection::RustConnection,
        root: u32,
        active: u32,
    ) -> u32 {
        let r = conn
            .get_property(false, root, active, 0u32, 0u32, 1u32)
            .unwrap()
            .reply()
            .unwrap();
        r.value32().and_then(|mut v| v.next()).unwrap_or(0)
    }
    let before = read_active(&conn, root, active);
    println!("active before: 0x{before:x}");
    let mut child = Command::new("./target/release/dbml-ui")
        .env("DISPLAY", std::env::var("DISPLAY")?)
        .spawn()?;
    let child_pid = child.id();
    let mut found = None;
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(250));
        let tree = conn.query_tree(root)?.reply()?;
        found = tree.children.iter().copied().find(|&x| {
            conn.get_property(false, x, pid_atom, 0u32, 0u32, 1u32)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().and_then(|mut v| v.next()))
                == Some(child_pid)
        });
        if found.is_some() {
            break;
        }
    }
    let Some(w) = found else {
        println!("app window never mapped");
        let _ = child.kill();
        return Ok(());
    };
    println!("app window: 0x{w:x} (mapped)");
    std::thread::sleep(Duration::from_millis(1500));
    let after = read_active(&conn, root, active);
    println!("active after:  0x{after:x}");
    // the active window may be our client, our client's WM frame, or a
    // sibling toplevel of the same process (GTK makes several; raise_self
    // targets the largest, the probe found the first)
    let tree = conn.query_tree(after).ok().and_then(|c| c.reply().ok());
    let frame_of_us = tree
        .as_ref()
        .map(|t| t.children.contains(&w))
        .unwrap_or(false);
    let pid_after = conn
        .get_property(false, after, pid_atom, 0u32, 0u32, 1u32)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().and_then(|mut v| v.next()));
    println!("active pid:    {pid_after:?} (app pid {child_pid})");
    let same = after == w;
    println!(
        "LAUNCH-ACTIVE: {}",
        if same || frame_of_us || pid_after == Some(child_pid) {
            "YES — raise_self works"
        } else {
            "NO"
        }
    );
    let _ = child.kill();
    Ok(())
}
