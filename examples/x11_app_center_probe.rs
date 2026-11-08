//! Diagnose the app's windows on the live session: every 500ms list all
//! toplevels belonging to the spawned app (by pid) or titled like it,
//! then (if found) move the main window to the right monitor and watch.
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
    let (net_name, wm_name, pid_atom) = (
        atom(b"_NET_WM_NAME")?,
        atom(b"WM_NAME")?,
        atom(b"_NET_WM_PID")?,
    );
    let title = |w: u32| -> Option<String> {
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
    let geom = |w: u32| -> Option<(i32, i32, u32, u32)> {
        let g = conn.get_geometry(w).ok()?.reply().ok()?;
        let a = conn.translate_coordinates(w, root, 0i16, 0i16).ok()?.reply().ok()?;
        Some((
            i32::from(a.dst_x),
            i32::from(a.dst_y),
            u32::from(g.width),
            u32::from(g.height),
        ))
    };
    let pid_of = |w: u32| -> Option<u32> {
        conn.get_property(false, w, pid_atom, 0u32, 0u32, 1u32)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next()))
    };
    let mut child = Command::new("./target/release/dbml-ui")
        .env("DISPLAY", std::env::var("DISPLAY")?)
        .spawn()?;
    let child_pid = child.id();
    println!("[probe] spawned app pid {child_pid}");
    let mut app: Option<u32> = None;
    for round in 0..30 {
        std::thread::sleep(Duration::from_millis(500));
        if child.try_wait()?.is_some() {
            println!("[{round}] APP EXITED");
            return Ok(());
        }
        let tree = conn.query_tree(root)?.reply()?;
        let children: Vec<u32> = tree.children.clone();
        let mut pid_hits = 0usize;
        let mut best: Option<(u64, u32)> = None;
        for &w in &children {
            let pid = pid_of(w);
            let t = title(w);
            let is_ours = pid == Some(child_pid);
            let looks_like = t
                .as_deref()
                .is_some_and(|n| n.to_lowercase().contains("dbml"));
            if is_ours || looks_like {
                pid_hits += usize::from(is_ours);
                println!(
                    "[{round}] 0x{w:x} pid={pid:?} {} {t:?} {:?}",
                    if is_ours { "OURS" } else { "titled" },
                    geom(w)
                );
            }
            if is_ours {
                if let Some((_, _, gw, gh)) = geom(w) {
                    let area = u64::from(gw) * u64::from(gh);
                    if best.is_none_or(|(a, _)| area > a) {
                        best = Some((area, w));
                    }
                }
            }
        }
        if app.is_none() {
            app = best.map(|(_, w)| w);
        }
        if app.is_some() && round >= 6 {
            break;
        }
    }
    let Some(app) = app else {
        println!("[probe] NO pid window seen in 15s — that is the bug");
        let _ = child.kill();
        return Ok(());
    };
    println!("[probe] main window 0x{app:x} — moving to the right monitor");
    conn.configure_window(
        app,
        &x11rb::protocol::xproto::ConfigureWindowAux::new()
            .x(1920)
            .y(0)
            .width(1920)
            .height(1000),
    )?;
    conn.flush()?;
    for round in 0..8 {
        std::thread::sleep(Duration::from_millis(500));
        println!(
            "[after-move {round}] alive={} pos={:?}",
            geom(app).is_some(),
            geom(app)
        );
        if let Some(st) = child.try_wait()? {
            println!("[after-move {round}] APP EXITED: {st}");
            return Ok(());
        }
    }
    println!("[probe] survived the move — the centering math would apply cleanly");
    let _ = child.kill();
    Ok(())
}
