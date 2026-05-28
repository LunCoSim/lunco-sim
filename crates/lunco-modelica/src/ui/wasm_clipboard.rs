//! Clipboard helpers for the code editor on wasm.
//!
//! ## The problem
//!
//! `bin/lunica.rs` configures Bevy's window with
//! `prevent_default_event_handling: true` so the browser doesn't
//! scroll / tab away on arrow keys etc. Side effect: bevy_winit
//! installs a canvas-level keydown listener that calls
//! `preventDefault()` on every key event, which means **the browser
//! never synthesises native `copy` / `cut` / `paste` events**.
//!
//! Without those events:
//!   - bevy_egui's built-in clipboard pipeline can't see anything.
//!   - `navigator.clipboard.readText()` is the only path left for
//!     paste — and the browser shows a permission prompt every time.
//!
//! ## What we do
//!
//! We selectively undo the `preventDefault` for *paste* keystrokes:
//!
//! 1. Capture-phase `keydown` listener on `document`: when the key
//!    is Ctrl/Cmd+V, call `stopImmediatePropagation()`. Capture phase
//!    fires before bevy_winit's canvas-level handler, and stopping
//!    propagation prevents that handler from running — so it never
//!    `preventDefault`s, and the browser proceeds to fire its native
//!    `paste` event.
//!
//! 2. Capture-phase `paste` listener on `document`: read the OS
//!    clipboard *synchronously* via `clipboardData.getData("text/
//!    plain")`. The browser allows this because we're inside a real
//!    paste gesture — no permission prompt. Park the text in
//!    `pending_paste` for the editor to apply on the next frame.
//!
//! 3. The menu's "Paste" entry has no underlying paste gesture, so
//!    it falls back to async `navigator.clipboard.readText()`. That
//!    *will* prompt the user — accepted as the cost of having a menu
//!    entry at all. Ctrl/Cmd+V is silent.
//!
//! Copy / Cut don't need any of this — `ctx.copy_text(...)` already
//! works through bevy_egui's `navigator.clipboard.writeText`, and
//! writes don't require a permission prompt in a user-gesture
//! context.

use bevy::prelude::*;

/// Bevy plugin: installs the document-level capture-phase listeners
/// on Startup. No-op on native.
pub struct WasmClipboardPlugin;

impl Plugin for WasmClipboardPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_arch = "wasm32")]
        app.add_systems(bevy::prelude::Startup, install_listeners);
        let _ = app;
    }
}

/// Stash clipboard text into the pending-paste slot so the editor
/// picks it up on the next frame.
///
/// - On wasm: spawn an async `navigator.clipboard.readText()`. The
///   keyboard Ctrl/Cmd+V path goes through the synchronous `paste`
///   event listener instead and never hits this; menu/toolbar buttons
///   have no real paste gesture so the browser prompts once.
/// - On native: read the OS clipboard synchronously via `arboard` and
///   stash the result. arboard is already in the dep tree (bevy_egui
///   uses it for its own Ctrl/Cmd+V handling).
pub fn request_paste_from_clipboard() {
    #[cfg(target_arch = "wasm32")]
    inner::request_paste_async();
    #[cfg(not(target_arch = "wasm32"))]
    native::request_paste_sync();
}

/// Drain a pending paste, if any. The editor calls this each frame
/// and inserts the returned text at the cursor (replacing any
/// selection).
pub fn take_pending_paste() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        inner::with_state(|s| s.pending_paste.take())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        native::take_pending_paste()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::Mutex;

    // Single global slot. The editor render runs on the main thread and
    // arboard is synchronous, so a `Mutex` is overkill in practice; using
    // it anyway to avoid relying on thread-local lifetimes when called
    // from inside an egui closure.
    static PENDING: Mutex<Option<String>> = Mutex::new(None);

    pub(super) fn request_paste_sync() {
        // Re-create the clipboard handle per call. arboard caches the
        // X11/Wayland/Cocoa/Windows connection internally, so this is
        // cheap, and avoids holding a long-lived handle that some
        // platforms (Wayland) dislike across compositor restarts.
        let text = match arboard::Clipboard::new()
            .and_then(|mut cb| cb.get_text())
        {
            Ok(t) => t,
            Err(e) => {
                bevy::log::warn!(
                    "[clipboard] paste failed: {e}"
                );
                return;
            }
        };
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(text);
        }
    }

    pub(super) fn take_pending_paste() -> Option<String> {
        PENDING.lock().ok().and_then(|mut slot| slot.take())
    }
}

#[cfg(target_arch = "wasm32")]
fn install_listeners() {
    inner::install();
}

#[cfg(target_arch = "wasm32")]
mod inner {
    use std::cell::RefCell;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    /// Wasm is single-threaded, so `RefCell` in a `thread_local!` is
    /// enough — JS callbacks and Bevy systems both run on the main
    /// thread and only one borrow is alive at a time.
    #[derive(Default)]
    pub(super) struct State {
        /// Filled by the `paste` JS listener (sync) or the async
        /// `readText` resolver (menu fallback). Drained by the
        /// editor system on the next frame.
        pub pending_paste: Option<String>,
    }

    thread_local! {
        pub(super) static STATE: RefCell<State> = RefCell::new(State::default());
        // Park closures here so they outlive `install()`. JS keeps
        // the function pointer; if Rust drops the `Closure`, the
        // listener calls into freed memory and crashes.
        static CLOSURES: RefCell<Vec<JsValue>> = RefCell::new(Vec::new());
    }

    pub(super) fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
        STATE.with(|s| f(&mut s.borrow_mut()))
    }

    pub(super) fn install() {
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };

        // Single shared options object; capture phase for both
        // listeners (see module doc for the full rationale).
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_capture(true);

        // ── 1. keydown: let the browser handle Ctrl/Cmd+V natively ──
        //
        // Without this, bevy_winit's canvas-level keydown listener
        // (registered later in the bubble phase) calls
        // `preventDefault()` and the browser never fires a `paste`
        // event. We fire first (capture phase, document target) and
        // call `stopImmediatePropagation()` to keep bevy_winit's
        // listener from running for this exact keystroke.
        //
        // Note: we DON'T call `preventDefault` ourselves — that
        // would suppress the very paste event we want. We just keep
        // bevy_winit's handler from suppressing it.
        let keydown_cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            |event: web_sys::KeyboardEvent| {
                let key = event.key();
                let is_v = key == "v" || key == "V";
                let cmd = event.ctrl_key() || event.meta_key();
                if is_v && cmd && !event.alt_key() {
                    event.stop_immediate_propagation();
                }
            },
        );
        let _ = document.add_event_listener_with_callback_and_add_event_listener_options(
            "keydown",
            keydown_cb.as_ref().unchecked_ref(),
            &opts,
        );

        // ── 2. paste: read the OS clipboard synchronously ──
        //
        // `getData("text/plain")` only works inside a real paste
        // event — the browser doesn't prompt because the user
        // already proved intent by pressing Ctrl+V. We grab the text,
        // hand it off to the editor via `pending_paste`, and
        // suppress propagation so bevy_egui's bubble-phase paste
        // listener doesn't *also* queue an `Event::Paste` that would
        // double-paste next frame.
        let paste_cb = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(
            |event: web_sys::ClipboardEvent| {
                let Some(cd) = event.clipboard_data() else { return };
                let Ok(text) = cd.get_data("text/plain") else { return };
                if text.is_empty() {
                    return;
                }
                with_state(|s| s.pending_paste = Some(text));
                event.prevent_default();
                event.stop_immediate_propagation();
            },
        );
        let _ = document.add_event_listener_with_callback_and_add_event_listener_options(
            "paste",
            paste_cb.as_ref().unchecked_ref(),
            &opts,
        );

        web_sys::console::log_1(
            &"[lunco wasm_clipboard] paste keydown+paste listeners installed".into(),
        );

        CLOSURES.with(|c| {
            let mut v = c.borrow_mut();
            v.push(keydown_cb.into_js_value());
            v.push(paste_cb.into_js_value());
        });
    }

    /// Async fallback for the menu's "Paste" entry — no real paste
    /// gesture exists from a click, so this is the only programmatic
    /// path. The browser prompts the user for clipboard read
    /// permission; on success we queue the text the same way the
    /// synchronous `paste` listener does. Some browsers (Firefox
    /// without the granted permission) reject — we log + drop.
    pub(super) fn request_paste_async() {
        let Some(window) = web_sys::window() else { return };
        let navigator = window.navigator();
        let clipboard = navigator.clipboard();
        let promise = clipboard.read_text();
        let future = JsFuture::from(promise);

        wasm_bindgen_futures::spawn_local(async move {
            match future.await {
                Ok(value) => {
                    if let Some(text) = value.as_string() {
                        if !text.is_empty() {
                            with_state(|s| s.pending_paste = Some(text));
                        }
                    }
                }
                Err(err) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[lunco wasm_clipboard] navigator.clipboard.readText() rejected: {:?}",
                        err
                    )));
                }
            }
        });
    }
}
