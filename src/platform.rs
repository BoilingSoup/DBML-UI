//! Platform integration: the physical-ctrl bridge.
//!
//! Some desktop environments consume a modifier key's own events (GNOME's
//! Locate Pointer grabs the Left-Ctrl keydown AND release), so toolkits
//! that track modifiers from received events both lose the flag on
//! subsequent letters AND can go stale after the grab. Terminals are
//! immune: the chord is encoded in the byte itself (kitty sends \u{12} for
//! Ctrl+R). The equivalent truth for a GUI app on X11 is the server's XKB
//! state machine: `XkbGetState`'s base/effective modifier state keeps
//! tracking the physical key STRAIGHT THROUGH the DE grab — live-verified
//! Oct 2026 (examples/xkb_probe.rs): during a grabbed Control_L hold, the
//! keymap bitmap answers "no ctrl" (and XI2 raw events are suspended
//! entirely — both were tried first), while XKB base_mods reads true the
//! whole hold and drops the instant the key releases. A stripped
//! ctrl-chord letter can therefore be bridged back to its chord at the
//! source instead of by after-the-fact salvage. Wayland needs none of
//! this (the compositor pushes modifier state with every key event), and
//! the bridge is inert when there is no X server.

use std::sync::atomic::{AtomicU32, Ordering};

/// Whether Control is PHYSICALLY held:
/// `Some(state)` on an X11 session — from the XKB state machine
/// (grab-proof; see the module doc), falling back to XI2 raw events and
/// then the keymap on servers lacking XKB — and `None` when there is no
/// X server (native Wayland, headless); the caller falls back to its
/// tracked switch.
pub fn ctrl_held() -> Option<bool> {
    use std::sync::OnceLock;
    // the Connection trait carries setup() + the xproto request methods
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xinput::ConnectionExt as _;
    use x11rb::protocol::xkb::ConnectionExt as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    // the bridge only makes sense on an X11 session: a native Wayland app
    // gets per-event modifier state from the compositor, and XWayland's
    // keymap says nothing about a Wayland client's keys
    static X11: OnceLock<Option<X11Keys>> = OnceLock::new();

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Session {
        X11,
        Other,
    }

    fn session_type() -> Session {
        match std::env::var("XDG_SESSION_TYPE").as_deref() {
            Ok("x11") => Session::X11,
            _ => Session::Other,
        }
    }

    struct X11Keys {
        conn: RustConnection,
        /// (byte index, bit mask) of every keycode mapped to Control_L/R
        /// — the keymap fallback (no XKB, no XI2).
        query_bits: Vec<(usize, u8)>,
        /// The same keys' KEYCODES, matched against XI2 raw events.
        keycodes: Vec<u16>,
        /// Bit N set while `keycodes[N]` is physically down per raw
        /// events; updated as events drain, read lock-free.
        raw_down: AtomicU32,
        /// The XI2 raw selection succeeded: `raw_down` is usable.
        raw_on: bool,
        /// XkbGetState is usable — the grab-proof truth (the module doc).
        xkb_on: bool,
    }

    /// XI2 device ids: all devices, masters and slaves alike (raw events
    /// ride the hardware; master-only selection can miss a grabbed key).
    const XI_ALL_DEVICES: u16 = 0;
    /// XKB's "use the core keyboard device" spec (XKB_USE_CORE_KBD).
    const XKB_USE_CORE_KBD: u16 = 0x0100;

    let x11 = X11.get_or_init(|| {
        if session_type() != Session::X11 {
            return None;
        }
        let (conn, screen) = x11rb::connect(None).ok()?;
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let count = (max - min + 1) as usize;
        if count == 0 {
            return None;
        }
        let map = conn
            .get_keyboard_mapping(min, count as u8)
            .ok()?
            .reply()
            .ok()?;
        let per = map.keysyms_per_keycode.max(1) as usize;
        // X keycodes start at 8; query_keymap's bitmap is indexed the same
        let mut query_bits = Vec::new();
        let mut keycodes = Vec::new();
        for kc in 0..count {
            let row = &map.keysyms[kc * per..(kc * per + per).min(map.keysyms.len())];
            if row.contains(&0xffe3) || row.contains(&0xffe4) {
                // Control_L / Control_R
                let keycode = min + kc as u8;
                let index = (keycode - 8) as usize;
                query_bits.push((index / 8, 1u8 << (index % 8)));
                keycodes.push(keycode as u16);
            }
        }
        if keycodes.is_empty() {
            return None;
        }
        // XI2 raw events on the ROOT window — the middle fallback (they
        // are SUSPENDED while a grab is active, which is why XKB leads).
        let root = conn.setup().roots.get(screen)?.root;
        let raw_on = (|| -> Option<()> {
            conn.xinput_xi_query_version(2, 0).ok()?.reply().ok()?;
            conn.xinput_xi_select_events(
                root,
                &[x11rb::protocol::xinput::EventMask {
                    deviceid: XI_ALL_DEVICES,
                    mask: vec![
                        x11rb::protocol::xinput::XIEventMask::RAW_KEY_PRESS,
                        x11rb::protocol::xinput::XIEventMask::RAW_KEY_RELEASE,
                    ],
                }],
            )
            .ok()?
            .check()
            .ok()?;
            Some(())
        })()
        .is_some();
        // XKB: initialize the extension, remember whether the server
        // actually supports it (every modern server does — XKB has been
        // mandatory since X11R6.4)
        let xkb_on = conn
            .xkb_use_extension(1, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.supported)
            .unwrap_or(false);
        Some(X11Keys {
            conn,
            query_bits,
            keycodes,
            raw_down: AtomicU32::new(0),
            raw_on,
            xkb_on,
        })
    });
    let Some(x11) = x11 else {
        return None;
    };
    // 1st truth: the XKB state machine — base/effective modifiers keep
    // counting the physical key through any active grab (module doc)
    if x11.xkb_on {
        if let Ok(state) = x11.conn.xkb_get_state(XKB_USE_CORE_KBD) {
            if let Ok(reply) = state.reply() {
                use x11rb::protocol::xproto::ModMask;
                let ctl = u32::from(ModMask::CONTROL);
                let held = u32::from(reply.base_mods) & ctl != 0
                    || u32::from(reply.mods) & ctl != 0;
                return Some(held);
            }
        }
    }
    // 2nd truth: XI2 raw events (works until a grab activates)
    if x11.raw_on {
        while let Ok(Some(ev)) = x11.conn.poll_for_event() {
            match ev {
                x11rb::protocol::Event::XinputRawKeyPress(e) => {
                    mark_ctrl(&x11.keycodes, &x11.raw_down, e.detail, true)
                }
                x11rb::protocol::Event::XinputRawKeyRelease(e) => {
                    mark_ctrl(&x11.keycodes, &x11.raw_down, e.detail, false)
                }
                _ => {}
            }
        }
        return Some(x11.raw_down.load(Ordering::Relaxed) != 0);
    }
    // last resort: the keymap bitmap — deceived during grabs (module doc)
    let Ok(cookie) = xproto::query_keymap(&x11.conn) else {
        return None;
    };
    let Ok(reply) = cookie.reply() else {
        return None;
    };
    Some(
        x11.query_bits
            .iter()
            .any(|(byte, mask)| reply.keys[*byte] & mask != 0),
    )
}

/// Fold one raw key event into the ctrl bitmask (a no-op for other keys;
/// a repeated press is idempotent). Raw detail is a u32-wrapped keycode.
fn mark_ctrl(keycodes: &[u16], raw_down: &AtomicU32, detail: u32, down: bool) {
    let Some(bit) = keycodes.iter().position(|&k| k as u32 == detail) else {
        return;
    };
    let mask = 1u32 << bit.min(31);
    if down {
        raw_down.fetch_or(mask, Ordering::Relaxed);
    } else {
        raw_down.fetch_and(!mask, Ordering::Relaxed);
    }
}

/// The xid of this process's MAIN top-level window: the largest
/// pid-matching (`_NET_WM_PID`), non-override-redirect toplevel whose
/// name does not carry `except` (the dialog itself belongs to our pid
/// too). None off X11 or when nothing matches.
fn own_window_xid(
    conn: &x11rb::rust_connection::RustConnection,
    root: u32,
    net_wm_pid_w: u32,
    wm_name_w: u32,
    net_wm_name_w: u32,
    except_contains: &str,
    pid: u32,
) -> Option<u32> {
    use x11rb::protocol::xproto::ConnectionExt as _;
    let tree = conn.query_tree(root).ok()?.reply().ok()?;
    let name_of = |w: u32| -> Option<String> {
        for prop in [net_wm_name_w, wm_name_w] {
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
    let mut best: Option<(u64, u32)> = None; // (area, xid)
    for &w in &tree.children {
        let Ok(attr) = conn.get_window_attributes(w).ok()?.reply() else {
            continue;
        };
        if attr.override_redirect {
            continue;
        }
        if name_of(w).is_some_and(|n| n.contains(except_contains)) {
            continue;
        }
        let Ok(r) = conn.get_property(false, w, net_wm_pid_w, 0u32, 0u32, 1).ok()?.reply() else {
            continue;
        };
        if r.value_len == 0 || r.value32().and_then(|mut v| v.next()) != Some(pid) {
            continue;
        }
        let g = conn.get_geometry(w).ok()?.reply().ok()?;
        let area = g.width as u64 * g.height as u64;
        if best.is_none_or(|(a, _)| area > a) {
            best = Some((area, w));
        }
    }
    best.map(|(_, w)| w)
}

/// Raise and focus the native save dialog: rfd's GTK dialog opens
/// WITHOUT a transient parent (the gtk3 backend ignores set_parent —
/// only the XDG-portal backend applies it), so the window manager
/// stacks the orphan by its own placement/focus-stealing rules — with a
/// maximized browser behind the app it lands BELOW the focused window
/// and stays there. This polls for a toplevel whose name contains
/// `title` and: (1) adopts it — WM_TRANSIENT_FOR pointing at the app's
/// own window, the EWMH relationship that pins a dialog above its
/// parent; (2) activates it (EWMH), then a plain raise + focus as the
/// fallback; (3) KEEPS enforcing the raise briefly — a WM processing
/// the activate after our configure may re-bury the still-orphan
/// dialog. X11 only — Wayland compositors place the dialog themselves.
/// Detached thread, self-terminating.
pub fn raise_dialog(title: &str) {
    if std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("x11") {
        return;
    }
    let needle = title.to_string();
    std::thread::spawn(move || {
        let Ok((conn, screen)) = x11rb::connect(None) else {
            return;
        };
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::ConnectionExt as _;
        let root = conn.setup().roots[screen].root;
        let Some((active_w, wm_name_w, net_wm_name_w, net_wm_pid_w, transient_w)) = (|| {
            Some((
                conn.intern_atom(false, b"_NET_ACTIVE_WINDOW").ok()?.reply().ok()?.atom,
                conn.intern_atom(false, b"WM_NAME").ok()?.reply().ok()?.atom,
                conn.intern_atom(false, b"_NET_WM_NAME").ok()?.reply().ok()?.atom,
                conn.intern_atom(false, b"_NET_WM_PID").ok()?.reply().ok()?.atom,
                conn.intern_atom(false, b"WM_TRANSIENT_FOR").ok()?.reply().ok()?.atom,
            ))
        })() else {
            return;
        };
        let name_of = |w: u32| -> Option<String> {
            for prop in [net_wm_name_w, wm_name_w] {
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
        let pid = std::process::id();
        // the dialog takes a beat to map; poll, adopt + raise on a hit,
        // then keep the dialog on top briefly (a WM that processes the
        // activate after our configure can re-bury it)
        let mut adopted = false;
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let Some(tree) = conn.query_tree(root).ok().and_then(|c| c.reply().ok())
            else {
                return;
            };
            let mut found = None;
            for &w in &tree.children {
                if name_of(w).is_some_and(|n| n.contains(&needle)) {
                    found = Some(w);
                    break;
                }
            }
            let Some(w) = found else {
                continue;
            };
            if !adopted {
                adopted = true;
                // adopt: transient-for the app's own window — the
                // relationship that makes every WM keep a dialog above
                // its parent (and groups it, so FSP stops treating it
                // as an unrelated new window)
                if let Some(app) = own_window_xid(
                    &conn, root, net_wm_pid_w, wm_name_w, net_wm_name_w, &needle, pid,
                ) {
                    use x11rb::wrapper::ConnectionExt as _;
                    let _ = conn.change_property32(
                        x11rb::protocol::xproto::PropMode::REPLACE,
                        w,
                        transient_w,
                        x11rb::protocol::xproto::AtomEnum::WINDOW,
                        &[app],
                    );
                }
                // EWMH activate (source 1 = the application)
                let data = x11rb::protocol::xproto::ClientMessageData::from([
                    w, 1u32, 0u32, 0u32, 0u32,
                ]);
                let msg = x11rb::protocol::xproto::ClientMessageEvent::new(
                    32u8,
                    w,
                    active_w,
                    data,
                );
                use x11rb::protocol::xproto::EventMask;
                let _ = conn.send_event(
                    false,
                    root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    msg,
                );
                let _ = conn.set_input_focus(
                    x11rb::protocol::xproto::InputFocus::PARENT,
                    w,
                    0u8,
                );
            }
            // the plain raise: re-asserted for a few seconds so a WM's
            // async re-stack cannot win
            let _ = conn.map_window(w);
            let _ = conn.configure_window(
                w,
                &x11rb::protocol::xproto::ConfigureWindowAux::new()
                    .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
            );
            let _ = conn.flush();
            if adopted {
                // adopted and raised: a few more enforcement passes,
                // then leave the dialog alone
                for _ in 0..10 {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = conn.configure_window(
                        w,
                        &x11rb::protocol::xproto::ConfigureWindowAux::new()
                            .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
                    );
                    let _ = conn.flush();
                }
                return;
            }
        }
    });
}

// xproto used by the fallback query above
use x11rb::protocol::xproto as xproto;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_raw_bitmask_tracks_each_ctrl_key_independently() {
        // two ctrl keys (Control_L=37, Control_R=105): pressing both then
        // releasing ONE keeps ctrl held — the per-keycode tracking the
        // raw path needs (a single bool would drop the hold)
        let keys: Vec<u16> = vec![37, 105];
        let state = AtomicU32::new(0);
        mark_ctrl(&keys, &state, 37, true);
        assert_eq!(state.load(Ordering::Relaxed), 1);
        mark_ctrl(&keys, &state, 105, true);
        assert_eq!(state.load(Ordering::Relaxed), 3);
        mark_ctrl(&keys, &state, 37, false);
        assert_eq!(
            state.load(Ordering::Relaxed),
            2,
            "Right ctrl still holds the chord"
        );
        // non-ctrl keys never touch the mask; repeats are idempotent
        mark_ctrl(&keys, &state, 24, true);
        mark_ctrl(&keys, &state, 105, true);
        assert_eq!(state.load(Ordering::Relaxed), 2);
        mark_ctrl(&keys, &state, 105, false);
        assert_eq!(state.load(Ordering::Relaxed), 0);
    }

    /// Live self-check (no injection available on this box): run the app
    /// with DBML_UI_KEY_DEBUG=1 and press ctrl — the [key] log lines show
    /// `bridge=Some(true)` while it is held, straight from XKB state.
    #[test]
    #[ignore = "manual: needs a human pressing ctrl; see the doc comment"]
    fn manual_ctrl_held_flips_live() {
        for _ in 0..100 {
            println!("ctrl_held = {:?}", ctrl_held());
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
