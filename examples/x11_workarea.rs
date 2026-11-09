//! Print _NET_WORKAREA and probe the left clamp: configure a zenity
//! client to x=0 via the production math, report where it lands.
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
    let workarea = atom(b"_NET_WORKAREA")?;
    let r = conn.get_property(false, root, workarea, 0u32, 0u32, 16u32)?
        .reply()?;
    let v: Vec<u32> = r.value32().map(|i| i.collect()).unwrap_or_default();
    println!("_NET_WORKAREA cardinals: {v:?}  (x, y, w, h per desktop)");
    let (net_name, wm_name, _pid_atom) = (
        atom(b"_NET_WM_NAME")?, atom(b"WM_NAME")?, atom(b"_NET_WM_PID")?,
    );
    let title = |w: u32| -> Option<String> {
        for prop in [net_name, wm_name] {
            if let Ok(r) = conn.get_property(false, w, prop, 0u32, 0u32, 1024u32) {
                if let Ok(v) = r.reply() {
                    let s = String::from_utf8_lossy(&v.value).trim().to_string();
                    if !s.is_empty() { return Some(s); }
                }
            }
        }
        None
    };
    let mut child = Command::new("zenity")
        .args(["--file-selection", "--title=zz-left-clamp"])
        .spawn()?;
    // two-level toplevel walk (frames -> clients)
    let toplevels = || -> Vec<u32> {
        let mut out = Vec::new();
        if let Ok(c) = conn.query_tree(root) { if let Ok(f) = c.reply() {
            for w in f.children {
                out.push(w);
                if let Ok(c2) = conn.query_tree(w) { if let Ok(t) = c2.reply() {
                    out.extend(t.children.iter().copied());
                }}
            }
        }}
        out
    };
    let mut d = None;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(100));
        d = toplevels().into_iter().find(|&w| title(w).is_some_and(|n| n.contains("zz-left-clamp")));
        if d.is_some() { break; }
    }
    let d = d.expect("dialog not found");
    let geom = |w: u32| -> Option<(i32, i32, u32, u32)> {
        let g = conn.get_geometry(w).ok()?.reply().ok()?;
        let a = conn.translate_coordinates(w, root, 0i16, 0i16).ok()?.reply().ok()?;
        Some((i32::from(a.dst_x), i32::from(a.dst_y), u32::from(g.width), u32::from(g.height)))
    };
    let (x0, y0, w, h) = geom(d).expect("geom");
    let parent = conn.query_tree(d)?.reply()?.parent;
    let pg = geom(parent);
    println!("dialog client: ({x0},{y0}) {w}x{h}  frame: {pg:?}");
    // production math targeting client-left at x=0
    let g = conn.get_geometry(d).unwrap().reply().unwrap();
    let abs = conn.translate_coordinates(d, root, 0i16, 0i16).unwrap().reply().unwrap();
    conn.configure_window(d, &x11rb::protocol::xproto::ConfigureWindowAux::new()
        .x(i32::from(g.x) + 0 - i32::from(abs.dst_x))
        .y(i32::from(g.y) + 0 - i32::from(abs.dst_y)))?;
    conn.flush()?;
    std::thread::sleep(Duration::from_millis(800));
    let (x1, y1, _, _) = geom(d).expect("geom");
    let pg2 = geom(parent);
    println!("after x=0 request: client ({x1},{y1})  frame: {pg2:?}");
    let _ = child.kill();
    Ok(())
}
