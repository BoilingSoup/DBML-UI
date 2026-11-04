//! Web-build persistence and file delivery.
//!
//! The browser has no file system and no reliable "saved" signal: a
//! download leaves the app the moment the user confirms it, and whether
//! it survives on disk is nobody's guess. So the web document model is
//! different from the desktop's:
//!
//! - the whole editor state (every tab's buffer, the active tab, the
//!   split) autosaves to IndexedDB — a couple of seconds after each
//!   edit, and again on tab switches and visibility changes;
//! - a page load restores that record (no restart gates, no GPU-loss
//!   special case: the web session IS the last known state);
//! - nothing tracks unsaved changes — the tab-close confirm and the
//!   dirty pill don't exist there, because "saved" is not a fact the
//!   app can know;
//! - Save As / Export produce a browser download: fire and forget.

use crate::session::{self, Session};
use std::cell::RefCell;
use std::rc::Rc;

use iced::futures::channel::oneshot;
use iced::futures::SinkExt;
use iced::futures::StreamExt;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

const DB_NAME: &str = "dbml-ui";
const STORE: &str = "state";
const KEY: &str = "session";
const CONFIG_KEY: &str = "config";

fn db_request() -> Result<(web_sys::IdbOpenDbRequest, Closure<dyn FnMut(web_sys::Event)>), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let factory = window
        .indexed_db()?
        .ok_or_else(|| JsValue::from_str("no indexed db"))?;
    let open: web_sys::IdbOpenDbRequest = factory
        .open_with_u32(DB_NAME, 1)?
        .unchecked_into();
    // first run: create the store (later runs keep it)
    let open_for_upgrade = open.clone();
    let on_upgrade = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
        if let Ok(db) = open_for_upgrade.result().map(|r| r.unchecked_into::<web_sys::IdbDatabase>()) {
            db.create_object_store(STORE).ok();
        }
    });
    open.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));
    Ok((open, on_upgrade))
}

/// The web boot record: the persisted settings (the config key=value
/// text) plus the last autosaved editor state. Carried inside a Message
/// (hence the derives) — the Session/Config parts are what matter.
#[derive(Debug, Clone, PartialEq)]
pub struct Boot {
    pub config: Option<String>,
    pub session: Option<Session>,
}

/// Load the boot record — the config first, then the state, so the
/// restored tabs come back in the persisted editing mode. A fresh
/// browser (or an unreadable record) boots bare: broken snapshots must
/// never block the boot.
pub fn load_boot() -> iced::Task<Boot> {
    let (tx, rx) = oneshot::channel::<(Option<String>, Option<String>)>();
    match load_inner(tx) {
        Ok(()) => iced::Task::perform(
            async move {
                let (config, session_raw) = rx.await.unwrap_or((None, None));
                Boot {
                    config,
                    session: session_raw
                        .and_then(|raw| session::decode(&raw))
                        // an all-splash record restores nothing
                        .filter(|s| s.tabs.iter().any(|t| t.doc_open)),
                }
            },
            |b| b,
        ),
        // no idb (private mode, old webview): the boot proceeds unsaved
        Err(_) => iced::Task::none(),
    }
}

fn load_inner(tx: oneshot::Sender<(Option<String>, Option<String>)>) -> Result<(), JsValue> {
    let (open, on_upgrade) = db_request()?;
    // the oneshot sender is consumed by `send`, so the callback passes it
    // along in an Option (this fires exactly once)
    let tx_cell = Rc::new(RefCell::new(Some(tx)));
    let open2 = open.clone();
    // the oneshot sender rides in a shared cell: the chained callbacks
    // hand it along without moving their captures (a closure that moves
    // a capture out is FnOnce, and wasm-bindgen needs FnMut)
    
    let on_success = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
        let Ok(db) = open2.result().map(|r| r.unchecked_into::<web_sys::IdbDatabase>())
        else {
            if let Some(tx) = tx_cell.borrow_mut().take() {
                let _ = tx.send((None, None));
            }
            return;
        };
        let t = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readonly);
        let Ok(t) = t else {
            if let Some(tx) = tx_cell.borrow_mut().take() {
                let _ = tx.send((None, None));
            }
            return;
        };
        let Ok(cfg_req) = t
            .object_store(STORE)
            .and_then(|store| store.get(&JsValue::from_str(CONFIG_KEY)))
        else {
            if let Some(tx) = tx_cell.borrow_mut().take() {
                let _ = tx.send((None, None));
            }
            return;
        };
        let Ok(sess_req) = t
            .object_store(STORE)
            .and_then(|store| store.get(&JsValue::from_str(KEY)))
        else {
            if let Some(tx) = tx_cell.borrow_mut().take() {
                let _ = tx.send((None, None));
            }
            return;
        };

        let cfg_req: web_sys::IdbRequest = cfg_req.unchecked_into();
        let cfg_req2 = cfg_req.clone();
        let sess_req: web_sys::IdbRequest = sess_req.unchecked_into();
        let sess_req2: web_sys::IdbRequest = sess_req.clone().unchecked_into();
        // the session read rides the config read's success; both handles
        // live outside the closures so the `move` closures CLONE their
        // handles instead of moving an enclosing closure's capture out
        // (a moved capture turns the outer closure into a FnOnce, which
        // wasm-bindgen's FnMut bound rejects)
        let sess_req3 = std::rc::Rc::new(sess_req.clone().unchecked_into::<web_sys::IdbRequest>());
        let tx2 = tx_cell.clone();
        let on_cfg = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            let config = cfg_req2.result().ok().and_then(|v| v.as_string());
            let sess = sess_req3.clone();
            let tx3 = tx2.clone();
            let on_sess = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
                let session = sess.result().ok().and_then(|v| v.as_string());
                if let Some(tx) = tx3.borrow_mut().take() {
                    let _ = tx.send((config.clone(), session));
                }
            });
            sess_req2.set_onsuccess(Some(on_sess.as_ref().unchecked_ref()));
            on_sess.forget();
        });
        cfg_req.set_onsuccess(Some(on_cfg.as_ref().unchecked_ref()));
        on_cfg.forget();
    });
    open.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
    on_upgrade.forget();
    on_success.forget();
    Ok(())
}

/// Autosave: write the encoded snapshot. Fire and forget — the next
/// successful save overwrites it, and a failed write loses nothing that
/// the user's next keystroke would not rebuild.
pub fn save_state_task(s: &Session) -> iced::Task<bool> {
    let raw = session::encode(s);
    // the pagehide/visibilitychange flush reads this stash: an event
    // handler cannot reach the app, so the latest encoded snapshot waits
    // here for it
    stash(raw.clone());
    let (tx, rx) = oneshot::channel::<bool>();
    // the sender rides a shared cell: the open's success starts the
    // write, the put's completion resolves the task — two callbacks, one
    // channel (a moved capture would turn a closure into a FnOnce, which
    // wasm-bindgen's FnMut bound rejects)
    let tx = Rc::new(RefCell::new(Some(tx)));
    let started = (|| {
        let (open, on_upgrade) = db_request()?;
        let open2 = open.clone();
        let tx2 = tx.clone();
        let on_fail = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            if let Some(tx) = tx2.borrow_mut().take() {
                let _ = tx.send(false);
            }
        });
        open.set_onerror(Some(on_fail.as_ref().unchecked_ref()));
        on_fail.forget();
        let on_open = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
            let Ok(db) = open2.result().map(|r| r.unchecked_into::<web_sys::IdbDatabase>())
            else {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(false);
                }
                return;
            };
            let put = db
                .transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite)
                .and_then(|t| t.object_store(STORE))
                .and_then(|store| {
                    // IDBObjectStore.put(value, key): the value first, the
                    // key second
                    store.put_with_key(&JsValue::from_str(&raw), &JsValue::from_str(KEY))
                });
            if let Ok(put) = put {
                let put: web_sys::IdbRequest = put.unchecked_into();
                let put2 = put.clone();
                let tx3 = tx.clone();
                let on_put = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
                    let ok = put2.result().is_ok();
                    if let Some(tx) = tx3.borrow_mut().take() {
                        let _ = tx.send(ok);
                    }
                });
                put.set_onsuccess(Some(on_put.as_ref().unchecked_ref()));
                put.set_onerror(Some(on_put.as_ref().unchecked_ref()));
                on_put.forget();
            } else if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(false);
            }
        });
        open.set_onsuccess(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();
        Ok::<(), JsValue>(())
    })();
    if started.is_err() {
        // no idb (private mode): report the failure — the toolbar shows
        // the state honestly instead of a spinner forever
        return iced::Task::perform(async { false }, |ok| ok);
    }
    iced::Task::perform(async move { rx.await.unwrap_or(false) }, |ok| ok)
}

/// The local wall clock as "hh:mm" (JS Date: local timezone, matching
/// the browser's own download timestamps).
pub fn clock_hh_mm() -> String {
    let d = js_sys::Date::new_0();
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

/// Open the browser's file chooser filtered to `accept` (e.g. ".dbml").
/// The picked file's (name, text content) comes back through the task;
/// `None` = the chooser was dismissed (or the read failed — a broken
/// file must never wedge the flow).
pub fn pick_file(accept: &'static str) -> iced::Task<Option<(String, String)>> {
    let (tx, rx) = oneshot::channel();
    if pick_file_inner(accept, tx).is_err() {
        return iced::Task::none();
    }
    iced::Task::perform(
        async move { rx.await.ok().flatten() },
        |picked| picked,
    )
}

fn pick_file_inner(accept: &'static str, tx: oneshot::Sender<Option<(String, String)>>) -> Result<(), JsValue> {
    let document = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?.document().ok_or_else(|| JsValue::from_str("no document"))?;
    let input: web_sys::HtmlInputElement = document
        .create_element("input")?
        .unchecked_into();
    input.set_type("file");
    input.set_accept(accept);
    input.style().set_property("display", "none").ok();
    let body = document.body().ok_or_else(|| JsValue::from_str("no body"))?;
    body.append_child(&input).ok();

    let mut tx = Some(tx);
    let input2 = input.clone();
    // the change event: read the first file as text and hand it back
    let on_change = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
        let Some(tx) = tx.take() else { return };
        let file = input2
            .files()
            .and_then(|list| list.get(0));
        let Some(file) = file else {
            let _ = tx.send(None); // dismissed
            return;
        };
        let name = file.name();
        let reader = match web_sys::FileReader::new() {
            Ok(r) => r,
            Err(_) => {
                let _ = tx.send(None);
                return;
            }
        };
        let reader2 = reader.clone();
        let mut tx = Some(tx);
        let on_load = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            let Some(tx) = tx.take() else { return };
            let content = reader2.result().ok().and_then(|v| v.as_string());
            let _ = tx.send(Some((name.clone(), content.unwrap_or_default())));
        });
        reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
        on_load.forget();
        let blob: &web_sys::Blob = file.unchecked_ref();
        reader.read_as_text(blob).ok();
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    input.click();
    body.remove_child(&input).ok();
    Ok(())
}

/// Install the window drag-and-drop handler: dropped `.dbml`/`.sql`
/// files stream back as (name, content) pairs. The `dragover`
/// cancellation is required for `drop` to fire at all.
pub fn dropped_files() -> iced::Subscription<(String, String)> {
    iced::Subscription::run_with_id(
        std::any::TypeId::of::<DropFiles>(),
        iced::stream::channel(4, |mut output| async move {
            let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
            if install_drop_listener(tx).is_err() {
                // no drop support (or no window): the subscription idles
                std::future::pending::<()>().await;
                unreachable!();
            }
            while let Some(picked) = rx.next().await {
                let _ = output.send(picked).await;
            }
        }),
    )
}

struct DropFiles;

fn install_drop_listener(
    tx: iced::futures::channel::mpsc::UnboundedSender<(String, String)>,
) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let tx = std::cell::RefCell::new(Some(tx));

    let on_dragover = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |e: web_sys::DragEvent| {
        e.prevent_default();
    });
    let window2 = window.clone();
    let on_drop = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |e: web_sys::DragEvent| {
        e.prevent_default();
        let Some(dt) = e.data_transfer() else { return };
        let Some(file) = dt.files().and_then(|list| list.get(0)) else { return };
        let name = file.name();
        let reader = match web_sys::FileReader::new() {
            Ok(r) => r,
            Err(_) => return,
        };
        let reader2 = reader.clone();
        let tx = tx.borrow();
        let Some(tx) = tx.as_ref() else { return };
        let tx = tx.clone();
        let on_load = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            let content = reader2.result().ok().and_then(|v| v.as_string()).unwrap_or_default();
            let _ = tx.unbounded_send((name.clone(), content));
        });
        reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
        on_load.forget();
        let blob: &web_sys::Blob = file.unchecked_ref();
        reader.read_as_text(blob).ok();
        let _ = window2; // keep the reference alive for the closure set
    });
    window
        .add_event_listener_with_callback("dragover", on_dragover.as_ref().unchecked_ref())?;
    window
        .add_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref())?;
    on_dragover.forget();
    on_drop.forget();
    Ok(())
}

/// Persist the settings text (the config key=value store) — fire and
/// forget like the state save.
pub fn save_config(text: &str) {
    if let Ok((open, on_upgrade)) = db_request() {
        let open2 = open.clone();
        let raw = text.to_string();
        let on_success = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            if let Ok(db) = open2.result().map(|r| r.unchecked_into::<web_sys::IdbDatabase>()) {
                if let Ok(t) = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite) {
                    if let Ok(store) = t.object_store(STORE) {
                        store.put_with_key(&JsValue::from_str(CONFIG_KEY), &JsValue::from_str(&raw)).ok();
                    }
                }
            }
        });
        open.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
        on_upgrade.forget();
        on_success.forget();
    }
}

/// The latest encoded snapshot, stashed on every autosave: the pagehide
/// flush reads this (the app cannot be reached from an event handler).
static STASH: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn stash(raw: String) {
    if let Ok(mut slot) = STASH.lock() {
        *slot = Some(raw);
    }
}

fn stashed() -> Option<String> {
    STASH.lock().ok().and_then(|slot| slot.clone())
}

/// Put the stashed snapshot straight into IndexedDB.
fn flush_stash() {
    if let Some(raw) = stashed() {
        if let Ok((open, on_upgrade)) = db_request() {
            let open2 = open.clone();
            let on_success = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
                if let Ok(db) = open2.result().map(|r| r.unchecked_into::<web_sys::IdbDatabase>()) {
                    if let Ok(t) = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite) {
                        if let Ok(store) = t.object_store(STORE) {
                            store.put_with_key(&JsValue::from_str(KEY), &JsValue::from_str(&raw)).ok();
                        }
                    }
                }
            });
            open.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
            on_upgrade.forget();
            on_success.forget();
        }
    }
}

/// Install the page-lifecycle flush: `pagehide` and `visibilitychange →
/// hidden` both fire BEFORE unload, while async writes can still land —
/// this closes the gap between the 2s autosave tick and a refresh.
pub fn install_flush_hook() {
    let Some(window) = web_sys::window() else { return };
    let on_pagehide = Closure::<dyn FnMut(web_sys::Event)>::new(|_e: web_sys::Event| {
        flush_stash();
    });
    let on_visibility = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        if let Some(d) = e.target().and_then(|t| t.dyn_into::<web_sys::Document>().ok()) {
            if d.visibility_state() == web_sys::VisibilityState::Hidden {
                flush_stash();
            }
        }
    });
    window
        .add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref())
        .ok();
    document().map(|d| {
        d.add_event_listener_with_callback(
            "visibilitychange",
            on_visibility.as_ref().unchecked_ref(),
        )
        .ok()
    });
    on_pagehide.forget();
    on_visibility.forget();
}

fn document() -> Option<web_sys::Document> {
    web_sys::window().and_then(|w| w.document())
}

/// Copy text to the system clipboard. Requires a user gesture for the
/// permission; the copy paths run inside key handlers, which qualify.
/// Fire and forget: a denied permission logs to the console (the app's
/// internal register always has the yank as a fallback).
pub fn copy_text(text: &str) {
    let Some(window) = web_sys::window() else { return };
    let clipboard = window.navigator().clipboard();
    let promise = clipboard.write_text(text);
    let on_err = Closure::<dyn FnMut(wasm_bindgen::JsValue)>::new(|err: wasm_bindgen::JsValue| {
        web_sys::console::warn_1(&format!("clipboard write failed: {err:?}").into());
    });
    // the write result is logged either way: a denied permission leaves
    // the internal register as the fallback (never a panic)
    let _ = promise.then2(&on_err, &on_err);
    on_err.forget();
}

/// Stream the browser's paste events (Ctrl+V / context-menu paste): the
/// DOM delivers the clipboard TEXT with the event — no async permission
/// dance, which readText cannot offer.
pub fn paste_events() -> iced::Subscription<String> {
    iced::Subscription::run_with_id(
        std::any::TypeId::of::<PasteEvents>(),
        iced::stream::channel(4, |mut output| async move {
            let (tx, mut rx) = iced::futures::channel::mpsc::unbounded();
            if install_paste_listener(tx).is_err() {
                std::future::pending::<()>().await;
                unreachable!();
            }
            while let Some(text) = rx.next().await {
                let _ = output.send(text).await;
            }
        }),
    )
}

struct PasteEvents;

fn install_paste_listener(
    tx: iced::futures::channel::mpsc::UnboundedSender<String>,
) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let tx = std::cell::RefCell::new(Some(tx));
    let on_paste = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |e: web_sys::ClipboardEvent| {
        let Some(dt) = e.clipboard_data() else {
            web_sys::console::log_1(&"paste: no clipboard data".into());
            return;
        };
        let Some(text) = dt.get_data("text/plain").ok().filter(|t| !t.is_empty()) else {
            web_sys::console::log_1(&"paste: empty text".into());
            return;
        };
        web_sys::console::log_1(&format!("paste: {} chars", text.len()).into());
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.unbounded_send(text);
        }
    });
    window
        .add_event_listener_with_callback("paste", on_paste.as_ref().unchecked_ref())?;
    on_paste.forget();
    Ok(())
}

/// Keep the browser tab's title in step with the app's (the active
/// tab's file, like the desktop window title).
pub fn set_title(title: &str) {
    if let Some(d) = document() {
        d.set_title(title);
    }
}

/// Hand `bytes` to the browser as a download named `name`. Fire and
/// forget: once the user confirms the download, success on their disk
/// is not something the app can know (and does not pretend to).
pub fn download(name: &str, mime: &str, bytes: &[u8]) {
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let arr = js_sys::Uint8Array::from(bytes);
    let seq = js_sys::Array::new();
    seq.push(&arr);
        let bag = {
        // set_type is deprecated in favor of a builder-style method that
        // takes self by value... but the deprecated one takes &self, so
        // bind the bag first
        let bag = web_sys::BlobPropertyBag::new();
        bag.set_type(mime);
        bag
    };
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&seq, &bag) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
    if let Ok(anchor) = document.create_element("a") {
        anchor.set_attribute("href", &url).ok();
        anchor.set_attribute("download", name).ok();
        if let Some(body) = document.body() {
            body.append_child(&anchor).ok();
            let html = anchor.clone().unchecked_into::<web_sys::HtmlElement>();
            html.click();
            body.remove_child(&anchor).ok();
        }
    }
    web_sys::Url::revoke_object_url(&url).ok();
}
