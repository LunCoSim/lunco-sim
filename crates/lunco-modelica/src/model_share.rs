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

// ─── Bevy/UI integration ──────────────────────────────────────────────
//
// `ModelSharePlugin` (the `CopyShareLink` clipboard observer + the wasm
// boot-time URL loader) lives in `crate::ui::model_share` — it touches UI
// surfaces (console, the `CreateNewScratchModel` command) and a workbench
// event, so it can't sit in this egui-free core module. The headless HTTP API
// reuses the pure `encode`/`decode`/`share_url` above directly.

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
