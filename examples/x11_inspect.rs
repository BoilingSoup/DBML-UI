//! Inspect windows by XID: name, pid, geometry, children. Args: ids...
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    };
    let (net_name, wm_name, pid_atom) = (
        atom(b"_NET_WM_NAME")?,
        atom(b"WM_NAME")?,
        atom(b"_NET_WM_PID")?,
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        // scan mode: list every toplevel with title/pid/geometry
        let tree = conn.query_tree(root)?.reply()?;
        for w in tree.children {
            let mut title = None;
            for prop in [net_name, wm_name] {
                if let Ok(r) = conn.get_property(false, w, prop, 0u32, 0u32, 1024u32) {
                    if let Ok(v) = r.reply() {
                        let s = String::from_utf8_lossy(&v.value).trim().to_string();
                        if !s.is_empty() {
                            title = Some(s);
                            break;
                        }
                    }
                }
            }
            let pid = conn
                .get_property(false, w, pid_atom, 0u32, 0u32, 1u32)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().and_then(|mut v| v.next()));
            let g = conn.get_geometry(w).ok().and_then(|c| c.reply().ok());
            println!(
                "0x{w:x}: {title:?} pid={pid:?} {:?}",
                g.map(|g| (g.x, g.y, g.width, g.height))
            );
        }
        return Ok(());
    }
    for arg in args {
        let w: u32 = u32::from_str_radix(arg.trim_start_matches("0x"), 16)?;
        let mut title = None;
        for prop in [net_name, wm_name] {
            if let Ok(r) = conn.get_property(false, w, prop, 0u32, 0u32, 1024u32) {
                if let Ok(v) = r.reply() {
                    let s = String::from_utf8_lossy(&v.value).trim().to_string();
                    if !s.is_empty() {
                        title = Some(s);
                        break;
                    }
                }
            }
        }
        let pid = conn
            .get_property(false, w, pid_atom, 0u32, 0u32, 1u32)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()));
        let g = conn.get_geometry(w).ok().and_then(|c| c.reply().ok());
        let tree = conn.query_tree(w).ok().and_then(|c| c.reply().ok());
        let parent = tree.as_ref().map(|t| t.parent);
        let kids = tree
            .map(|t| t.children.iter().map(|c| format!("0x{c:x}")).collect::<Vec<_>>())
            .unwrap_or_default();
        println!(
            "0x{w:x}: title={title:?} pid={pid:?} geom={:?} parent={:?} children={:?}",
            g.map(|g| (g.x, g.y, g.width, g.height)),
            parent.map(|p| format!("0x{p:x}")),
            kids
        );
    }
    let _ = root;
    Ok(())
}
