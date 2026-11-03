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
use iced::futures::channel::oneshot;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

const DB_NAME: &str = "dbml-ui";
const STORE: &str = "state";
const KEY: &str = "session";

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

/// Load the last autosaved state, or `None` on a fresh browser (or an
/// unreadable record — a broken snapshot must never block the boot).
pub fn load_state() -> iced::Task<Option<Session>> {
    let (tx, rx) = oneshot::channel::<Option<String>>();
    match load_inner(tx) {
        Ok(()) => iced::Task::perform(
            async move {
                rx.await
                    .ok()
                    .flatten()
                    .and_then(|raw| session::decode(&raw))
                    // an all-splash record restores nothing: boot bare
                    .filter(|s| s.tabs.iter().any(|t| t.doc_open))
            },
            |s| s,
        ),
        // no idb (private mode, old webview): the boot proceeds unsaved
        Err(_) => iced::Task::none(),
    }
}

fn load_inner(tx: oneshot::Sender<Option<String>>) -> Result<(), JsValue> {
    let (open, on_upgrade) = db_request()?;
    // the oneshot sender is consumed by `send`, so the callbacks pass it
    // along in an Option (each fires at most once in practice)
    let mut tx = Some(tx);
    let open2 = open.clone();
    let on_success = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
        let get = open2
            .result()
            .ok()
            .and_then(|r| {
                r.unchecked_into::<web_sys::IdbDatabase>()
                    .transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readonly)
                    .ok()
            })
            .and_then(|t| t.object_store(STORE).ok())
            .and_then(|store| store.get(&JsValue::from_str(KEY)).ok());
        let Some(get) = get else {
            // no store (or the transaction failed): boot bare
            if let Some(tx) = tx.take() {
                let _ = tx.send(None);
            }
            return;
        };
        let mut tx = tx.take();
        let get2 = get.clone();
        let on_get = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            let raw = get2.result().ok().and_then(|v| v.as_string());
            if let Some(tx) = tx.take() {
                let _ = tx.send(raw);
            }
        });
        get.set_onsuccess(Some(on_get.as_ref().unchecked_ref()));
        on_get.forget();
    });
    open.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
    on_upgrade.forget();
    on_success.forget();
    Ok(())
}

/// Autosave: write the encoded snapshot. Fire and forget — the next
/// successful save overwrites it, and a failed write loses nothing that
/// the user's next keystroke would not rebuild.
pub fn save_state(s: &Session) {
    let raw = session::encode(s);
    if let Ok((open, on_upgrade)) = db_request() {
        let open2 = open.clone();
        let on_success = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e| {
            if let Ok(db) = open2.result().map(|r| r.unchecked_into::<web_sys::IdbDatabase>()) {
                if let Ok(t) = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite) {
                    if let Ok(store) = t.object_store(STORE) {
                        // IDBObjectStore.put(value, key): the value first, the key second
store.put_with_key(&JsValue::from_str(&raw), &JsValue::from_str(KEY)).ok();
                    }
                }
            }
        });
        open.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
        on_upgrade.forget();
        on_success.forget();
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
    let mut bag = web_sys::BlobPropertyBag::new();
    bag.set_type(mime);
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
