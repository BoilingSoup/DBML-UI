//! Which move mechanism does THIS session's WM honor for a foreign
//! window? Tries, in order: plain ConfigureWindow, ConfigureWindow with
//! size too, _NET_MOVERESIZE_WINDOW (pager/NW), _NET_MOVERESIZE_WINDOW
//! (application/NW). Prints geometry after each.
use std::process::Command;
use std::time::Duration;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::protocol::xproto::EventMask;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    };
    let (net_name, wm_name, moveresize) = (
        atom(b"_NET_WM_NAME")?,
        atom(b"WM_NAME")?,
        atom(b"_NET_MOVERESIZE_WINDOW")?,
    );
    let mut child = Command::new("zenity")
        .args(["--file-selection", "--title=zz-center-probe"])
        .spawn()?;
    let name_of = |w: u32| -> Option<String> {
        for prop in [net_name, wm_name] {
            if let Ok(r) = conn.get_property(false, w, prop, 0u32, 0u32, 1024u32) {
                if let Ok(v) = r.reply() {
                    let s = String::from_utf8_lossy(&v.value).trim().to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }
        None
    };
    let mut w = None;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(150));
        let tree = conn.query_tree(root)?.reply()?;
        w = tree
            .children
            .iter()
            .copied()
            .find(|&x| name_of(x).is_some_and(|n| n.contains("zz-center-probe")));
        if w.is_some() { break; }
    }
    let w = w.expect("probe dialog never mapped");
    fn geom(
        conn: &x11rb::rust_connection::RustConnection,
        root: u32,
        w: u32,
    ) -> Option<(i32, i32, u32, u32)> {
        let g = conn.get_geometry(w).ok()?.reply().ok()?;
        let a = conn.translate_coordinates(w, root, 0i16, 0i16).ok()?.reply().ok()?;
        Some((
            i32::from(a.dst_x),
            i32::from(a.dst_y),
            u32::from(g.width),
            u32::from(g.height),
        ))
    }
    let (_, _, gw, gh) = geom(&conn, root, w).expect("geometry");
    let settle = |tag: &str| {
        std::thread::sleep(Duration::from_millis(900));
        let (x, y, _, _) = geom(&conn, root, w).expect("geometry");
        println!("{tag}: x={x} y={y}");
    };
    // 1: plain ConfigureWindow, root coords as parent-relative
    conn.configure_window(w, &x11rb::protocol::xproto::ConfigureWindowAux::new().x(500).y(400))?;
    conn.flush()?;
    settle("1 plain configure      ");
    // 2: with size echoed
    conn.configure_window(
        w,
        &x11rb::protocol::xproto::ConfigureWindowAux::new()
            .x(600)
            .y(420)
            .width(gw)
            .height(gh),
    )?;
    conn.flush()?;
    settle("2 configure + size     ");
    // 3: _NET_MOVERESIZE_WINDOW pager / NorthWest
    let send = |src: u32, gravity: u32, flags: u32, x: u32, y: u32| -> Result<(), Box<dyn std::error::Error>> {
        let data = x11rb::protocol::xproto::ClientMessageData::from([
            (src << 12) | gravity,
            flags,
            x,
            y,
            0,
        ]);
        let msg = x11rb::protocol::xproto::ClientMessageEvent::new(32u8, w, moveresize, data);
        conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            msg,
        )?;
        conn.flush()?;
        Ok(())
    };
    send(2, 1, 0b0011, 700, 440)?;
    settle("3 moveresize pager/NW  ");
    // 4: application / NorthWest
    send(1, 1, 0b0011, 800, 460)?;
    settle("4 moveresize app/NW    ");
    let _ = child.kill();
    Ok(())
}
