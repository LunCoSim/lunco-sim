//! Bevy/UI integration for shareable model links.
//!
//! The wire format + URL builder ([`encode`](crate::model_share::encode) /
//! [`decode`](crate::model_share::decode) /
//! [`share_url`](crate::model_share::share_url)) are egui-free core in
//! [`crate::model_share`] — the headless HTTP API reuses them via the
//! `CopyShareLink` query provider. What lives here is the *interactive* half:
//! the clipboard command observer and the wasm boot-time loader, both of which
//! touch UI surfaces (console, the `CreateNewScratchModel` command) and a
//! workbench event. A `--no-ui` server doesn't install this.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands};
use lunco_workbench::file_ops::CopyShareLink;

use crate::model_share::share_url;

// The typed struct is owned by `lunco-workbench` (so HTTP-API introspection sees
// the verb even in a `--no-ui` server that never installs this plugin); the
// observer that actually touches the clipboard lives here, and registers itself
// the same way as every other command.
register_commands!(on_copy_share_link);

/// Wires the `CopyShareLink` command observer (all platforms) and, on wasm,
/// the boot-time "open the model in the URL" system. Add after the Modelica
/// plugin so the document registry it reads already exists.
pub struct ModelSharePlugin;

impl Plugin for ModelSharePlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
        #[cfg(target_arch = "wasm32")]
        app.add_systems(Startup, load_shared_model_on_boot);
    }
}

/// Resolve the active document's share URL, if there is an active model.
fn active_share_url(world: &mut World) -> Option<String> {
    let doc_id = world
        .get_resource::<lunco_workspace::WorkspaceResource>()?
        .active_document?;
    let source = world
        .get_resource::<crate::state::ModelicaDocumentRegistry>()?
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
#[on_command(CopyShareLink)]
fn on_copy_share_link(trigger: On<CopyShareLink>, mut commands: Commands) {
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
            console.info(format!(
                "Share link copied to clipboard ({} chars)",
                url.len()
            ));
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
    crate::model_share::decode(&params.get(crate::model_share::FRAGMENT_KEY)?)
}

/// On boot, if the URL fragment carries a model, open it as a fresh
/// Untitled document by firing the existing creation command.
#[cfg(target_arch = "wasm32")]
fn load_shared_model_on_boot(mut commands: Commands) {
    let Some(source) = shared_source_from_url() else {
        return;
    };
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
