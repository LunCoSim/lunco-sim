//! Cross-domain URI registry — one scheme, one handler, one dispatch.
//!
//! Modelica cross-references (`modelica://Modelica.Blocks.Examples.PID`),
//! future USD prim refs (`usd://path/to/stage.usd@</World/Foo>`), and
//! SysML v2 element refs (`sysml://package::Element`) all want the same
//! UX: a clickable link in a document that lands the user on the right
//! place. Rather than each domain wiring its own regex + `<a href>`
//! rewriting into every HTML renderer, the workbench owns a registry
//! of scheme handlers and a single dispatch entry point.
//!
//! ## Shape
//!
//! - [`UriHandler`] — a small trait every domain crate implements once.
//!   Returns a [`UriResolution`] describing what clicking the link
//!   should do (open a document, open a resource, navigate an anchor,
//!   delegate to the OS browser). Handlers are free to do further
//!   parsing internally — this registry cares only about the scheme
//!   prefix.
//!
//! - [`UriRegistry`] — a Bevy [`Resource`] holding the `scheme → handler`
//!   map. Populated by each domain plugin on `build`.
//!
//! - [`UriClicked`] — the event a link widget fires when the user
//!   clicks. Domain crates observe it and turn the resolution into
//!   their own concrete command (e.g. `OpenClass` for Modelica).
//!
//! ## Boundaries
//!
//! The workbench stays domain-free: it holds the registry and knows
//! the common resolution shapes. Domain crates (lunco-modelica, a
//! future lunco-usd, etc.) bring their own handler + observer that
//! translates the generic `UriResolution` into their concrete
//! behaviour. Adding a new scheme is registering one handler and one
//! observer; no edits to this file.
//!
//! ## Non-goals
//!
//! - Async resolution. Handlers return immediately; if a fetch is
//!   needed, the downstream command can kick off a background task.
//!   Keeps the widget layer simple (click → dispatch → event → done).
//! - Query/fragment grammar per scheme. The resolution enum carries
//!   the matched `identifier`/`anchor` verbatim; each handler parses
//!   whatever subset of the URI grammar it cares about.

use bevy::prelude::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The outcome of resolving a URI. Handlers build one of these; the
/// widget layer turns it into a [`UriClicked`] event for the domain
/// observer to act on.
///
/// Kept deliberately small — the union of "things a click on a link
/// inside an engineering document typically means". Add a variant
/// only when the existing four don't express an intent cleanly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UriResolution {
    /// Open (or focus) a document identified by an opaque string the
    /// owning domain understands. `doc_kind` is the domain tag the
    /// domain observer matches on — typically the scheme, e.g.
    /// `"modelica"`, `"usd"`.
    OpenDocument {
        /// Domain tag matching the emitting handler's scheme.
        doc_kind: &'static str,
        /// Domain-opaque identifier (fully qualified class name,
        /// prim path, whatever the handler needs).
        identifier: String,
    },
    /// Open a file-system resource — used for things like
    /// `modelica://Package/Resources/Images/foo.png` or
    /// `usd://.../texture.exr`.
    OpenResource {
        /// Absolute path the handler resolved from the URI. The
        /// workbench's image/file loaders take over from here.
        path: PathBuf,
    },
    /// Navigate to an anchor inside an already-open document (e.g. a
    /// `#Section2` fragment in a Documentation block).
    NavigateAnchor {
        /// Document identifier in the same form `OpenDocument` emits
        /// — the observer looks it up on its own document registry.
        identifier: String,
        /// Anchor name without the leading `#`.
        anchor: String,
    },
    /// The URI should open in the OS browser (e.g. an `https://...`
    /// link in Documentation). Handlers typically emit this for the
    /// fallback `http`/`https`/`mailto` schemes.
    External {
        /// Full URL to open.
        url: String,
    },
    /// The URI didn't match any handler — the widget layer treats
    /// this as an inert link.
    NotHandled,
}

/// A domain-provided resolver for a single URI scheme.
///
/// Keep handlers stateless — they're stored behind an `Arc` in the
/// registry and may be called from any system. If a handler needs
/// state (e.g. the current Twin root for resolving relative paths),
/// read it from the `World` in the observer that processes the
/// emitted `UriClicked` event, not inside `resolve`.
pub trait UriHandler: Send + Sync + 'static {
    /// Scheme without the `://` suffix. Lower-case; the registry
    /// matches case-insensitively on the incoming URI.
    fn scheme(&self) -> &'static str;

    /// Turn a full URI string (including scheme) into a resolution.
    /// Called on the main thread; must be cheap (no I/O beyond
    /// filesystem checks).
    fn resolve(&self, uri: &str) -> UriResolution;
}

/// Registry of scheme handlers. One per app, inserted by
/// [`WorkbenchPlugin`](crate::WorkbenchPlugin).
///
/// Domain crates register from their own plugin's `build`:
///
/// ```ignore
/// app.world_mut()
///     .resource_mut::<UriRegistry>()
///     .register(Arc::new(ModelicaUriHandler));
/// ```
#[derive(Resource, Default)]
pub struct UriRegistry {
    by_scheme: HashMap<String, Arc<dyn UriHandler>>,
}

impl UriRegistry {
    /// Install a handler. Replaces any previously registered handler
    /// for the same scheme (last-write-wins — lets tests swap
    /// handlers for a fake without unregistration ceremony).
    pub fn register(&mut self, handler: Arc<dyn UriHandler>) {
        self.by_scheme
            .insert(handler.scheme().to_ascii_lowercase(), handler);
    }

    /// Dispatch a URI to the handler owning its scheme. Returns
    /// [`UriResolution::NotHandled`] when no handler matches —
    /// including malformed input — so callers only need one branch
    /// for the unhappy path.
    pub fn dispatch(&self, uri: &str) -> UriResolution {
        let Some(scheme_end) = uri.find("://") else {
            return UriResolution::NotHandled;
        };
        let scheme = uri[..scheme_end].to_ascii_lowercase();
        match self.by_scheme.get(&scheme) {
            Some(h) => h.resolve(uri),
            None => UriResolution::NotHandled,
        }
    }

    /// Iterate registered schemes — useful for debug overlays or
    /// "what links am I actually rendering" dev tooling.
    pub fn schemes(&self) -> impl Iterator<Item = &str> {
        self.by_scheme.keys().map(String::as_str)
    }
}

/// Event fired when a URI-rendering widget is clicked. Domain
/// observers match on `resolution` to dispatch their own concrete
/// action — for example the Modelica observer handles
/// `OpenDocument { doc_kind: "modelica", .. }` by triggering
/// [`crate::OpenTab`] (or a Modelica-specific `OpenClass`) event.
///
/// The original URI string is kept on the event so observers can log
/// or surface it in errors without rebuilding from the resolution.
#[derive(Event, Clone, Debug)]
pub struct UriClicked {
    /// Full URI as rendered (preserves scheme + query + fragment).
    pub uri: String,
    /// Resolution the registry produced. Observers ignore variants
    /// they don't handle so multiple handlers can coexist for the
    /// same URI (e.g. analytics + the real opener).
    pub resolution: UriResolution,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeHandler;
    impl UriHandler for FakeHandler {
        fn scheme(&self) -> &'static str {
            "fake"
        }
        fn resolve(&self, uri: &str) -> UriResolution {
            UriResolution::OpenDocument {
                doc_kind: "fake",
                identifier: uri
                    .strip_prefix("fake://")
                    .unwrap_or(uri)
                    .to_string(),
            }
        }
    }

    #[test]
    fn unknown_scheme_returns_not_handled() {
        let reg = UriRegistry::default();
        assert_eq!(
            reg.dispatch("modelica://Foo"),
            UriResolution::NotHandled
        );
    }

    #[test]
    fn missing_scheme_delim_returns_not_handled() {
        let reg = UriRegistry::default();
        assert_eq!(reg.dispatch("just a string"), UriResolution::NotHandled);
    }

    #[test]
    fn registered_scheme_dispatches() {
        let mut reg = UriRegistry::default();
        reg.register(Arc::new(FakeHandler));
        assert_eq!(
            reg.dispatch("fake://hello"),
            UriResolution::OpenDocument {
                doc_kind: "fake",
                identifier: "hello".to_string()
            }
        );
    }

    #[test]
    fn scheme_match_is_case_insensitive() {
        let mut reg = UriRegistry::default();
        reg.register(Arc::new(FakeHandler));
        assert!(matches!(
            reg.dispatch("FAKE://x"),
            UriResolution::OpenDocument { .. }
        ));
    }

    #[test]
    fn last_register_wins_for_same_scheme() {
        struct Other;
        impl UriHandler for Other {
            fn scheme(&self) -> &'static str {
                "fake"
            }
            fn resolve(&self, _: &str) -> UriResolution {
                UriResolution::External {
                    url: "replaced".to_string(),
                }
            }
        }
        let mut reg = UriRegistry::default();
        reg.register(Arc::new(FakeHandler));
        reg.register(Arc::new(Other));
        assert_eq!(
            reg.dispatch("fake://x"),
            UriResolution::External {
                url: "replaced".to_string()
            }
        );
    }
}
