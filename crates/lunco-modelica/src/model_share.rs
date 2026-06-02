//! Shareable model links — carry a model's source inline in the page URL.
//!
//! A model is shared by base64url-encoding its full source into the URL
//! **fragment** (`…/#model=<token>`). The fragment never leaves the
//! browser, so sharing is a pure client-side round-trip: no upload, no
//! server storage, no account. Opening such a link spins up a fresh
//! editable Untitled document seeded with the decoded source.
//!
//! The module has two halves:
//!
//! - a tiny **pure core** ([`encode`] / [`decode`] / [`share_url`]) that
//!   defines the wire format and is the single source of truth shared by
//!   the clipboard command, the HTTP API (`CopyShareLink` query
//!   provider), and the boot-time loader; and
//! - the **Bevy integration** ([`ModelSharePlugin`]): an observer for the
//!   `CopyShareLink` command (copies the link to the clipboard) and, on
//!   wasm, a startup system that opens whatever model the URL carries.
//!
//! Loading does **not** reimplement document creation — it fires the
//! existing [`CreateNewScratchModel`](crate::ui::commands::CreateNewScratchModel)
//! command with the decoded `source`, reusing the one creation + tab-open
//! pipeline every "New model" entry point already funnels through.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use bevy::prelude::*;

/// URL-fragment parameter key carrying the encoded model source.
pub const FRAGMENT_KEY: &str = "model";

/// Public web origin used when building a link from a context that has no
/// `window.location` of its own — i.e. the native desktop app and the
/// headless HTTP API. The browser build substitutes the live location
/// (see [`share_url`]) so self-hosted / preview deployments stay correct.
pub const PUBLIC_BASE: &str = "https://lunica.lunco.space/";

/// Encode model source into a URL-fragment token (base64url, unpadded).
pub fn encode(source: &str) -> String {
    URL_SAFE_NO_PAD.encode(source.as_bytes())
}

/// Decode a fragment token back into model source. Returns `None` when
/// the token isn't valid base64url or the bytes aren't valid UTF-8.
pub fn decode(token: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(token.as_bytes()).ok()?;
    String::from_utf8(bytes).ok()
}

/// Build the full shareable URL for a model's `source`.
///
/// On wasm the base is the live page (`origin + pathname`) so the link
/// points back at the same deployment the user is on; everywhere else it
/// falls back to [`PUBLIC_BASE`].
#[cfg(target_arch = "wasm32")]
pub fn share_url(source: &str) -> String {
    let base = web_sys::window()
        .map(|w| w.location())
        .and_then(|loc| Some(format!("{}{}", loc.origin().ok()?, loc.pathname().ok()?)))
        .unwrap_or_else(|| PUBLIC_BASE.to_string());
    format!("{base}#{FRAGMENT_KEY}={}", encode(source))
}

/// Build the full shareable URL for a model's `source` (native / API).
#[cfg(not(target_arch = "wasm32"))]
pub fn share_url(source: &str) -> String {
    format!("{PUBLIC_BASE}#{FRAGMENT_KEY}={}", encode(source))
}

// ─── Bevy integration ─────────────────────────────────────────────────

/// Wires the `CopyShareLink` command observer (all platforms) and, on
/// wasm, the boot-time "open the model in the URL" system. Add after the
/// Modelica plugin so the document registry it reads already exists.
pub struct ModelSharePlugin;

impl Plugin for ModelSharePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_copy_share_link);
        #[cfg(target_arch = "wasm32")]
        app.add_systems(Startup, load_shared_model_on_boot);
    }
}

/// Resolve the active document's share URL, if there is an active model.
fn active_share_url(world: &mut World) -> Option<String> {
    let doc_id = world
        .get_resource::<lunco_workbench::WorkspaceResource>()?
        .active_document?;
    let source = world
        .get_resource::<crate::ui::ModelicaDocumentRegistry>()?
        .host(doc_id)?
        .document()
        .source()
        .to_string();
    Some(share_url(&source))
}

/// `CopyShareLink` handler: copy the active model's share URL to the OS /
/// browser clipboard. The HTTP API exposes the same verb as a query that
/// *returns* the URL instead (see `api_queries::CopyShareLinkProvider`),
/// since a headless server has no clipboard.
fn on_copy_share_link(
    _trigger: On<lunco_workbench::file_ops::CopyShareLink>,
    mut commands: Commands,
) {
    commands.queue(|world: &mut World| {
        let Some(url) = active_share_url(world) else {
            if let Some(mut console) =
                world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
            {
                console.warn("Copy Share Link: no active model to share");
            }
            return;
        };
        copy_to_clipboard(&url);
        if let Some(mut console) =
            world.get_resource_mut::<crate::ui::panels::console::ConsoleLog>()
        {
            console.info(format!("Share link copied to clipboard ({} chars)", url.len()));
        }
    });
}

/// Copy `text` to the OS clipboard (native, via `arboard`).
#[cfg(not(target_arch = "wasm32"))]
fn copy_to_clipboard(text: &str) {
    match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text.to_string())) {
        Ok(()) => {}
        Err(e) => bevy::log::warn!("[model_share] clipboard write failed: {e}"),
    }
}

/// Copy `text` to the browser clipboard (`navigator.clipboard.writeText`).
/// Fire-and-forget: in a real user gesture (a menu click) the browser
/// grants the write without a prompt.
#[cfg(target_arch = "wasm32")]
fn copy_to_clipboard(text: &str) {
    let Some(win) = web_sys::window() else { return };
    let promise = win.navigator().clipboard().write_text(text);
    wasm_bindgen_futures::spawn_local(async move {
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    });
}

/// Read a shared model out of the current page URL fragment, if present.
#[cfg(target_arch = "wasm32")]
fn shared_source_from_url() -> Option<String> {
    let hash = web_sys::window()?.location().hash().ok()?;
    let hash = hash.strip_prefix('#').unwrap_or(&hash);
    let params = web_sys::UrlSearchParams::new_with_str(hash).ok()?;
    decode(&params.get(FRAGMENT_KEY)?)
}

/// On boot, if the URL fragment carries a model, open it as a fresh
/// Untitled document by firing the existing creation command.
#[cfg(target_arch = "wasm32")]
fn load_shared_model_on_boot(mut commands: Commands) {
    let Some(source) = shared_source_from_url() else { return };
    let name = crate::extract_model_name(&source);
    bevy::log::info!(
        "[model_share] opening shared model from URL fragment ({} bytes)",
        source.len()
    );
    commands.trigger(crate::ui::commands::CreateNewScratchModel {
        source: Some(source),
        name,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_source() {
        let src = "model Foo\n  Real x(start = 1.0);\nequation\n  der(x) = -x;\nend Foo;\n";
        let token = encode(src);
        // base64url must be URL-fragment-safe: no '+', '/', '=', or space.
        assert!(!token.contains(['+', '/', '=', ' ']));
        assert_eq!(decode(&token).as_deref(), Some(src));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert_eq!(decode("not valid base64!!!"), None);
    }

    #[test]
    fn share_url_carries_the_token() {
        let url = share_url("model M end M;");
        let token = encode("model M end M;");
        assert!(url.ends_with(&format!("#{FRAGMENT_KEY}={token}")));
    }
}
