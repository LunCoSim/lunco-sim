//! Single-instance deep-link forwarding (native).
//!
//! When the OS hands a clicked `luncosim://connect?…` link to a *fresh* process
//! (the registered scheme handler always launches the binary), we want it to
//! land in the **already-running** LunCoSim rather than spawning a second window.
//! [`acquire`] makes that decision at startup over a local socket (Unix domain
//! socket / Windows named pipe, via `interprocess`):
//!
//! - if an instance is already listening → write the link to it and report
//!   [`LaunchOutcome::Forwarded`] (the caller exits);
//! - otherwise → bind the socket, become the primary, and report
//!   [`LaunchOutcome::Primary`] with a [`DeepLinkInbox`] that a Bevy system
//!   drains into [`PendingConnect`](lunco_core::session::PendingConnect).
//!
//! The link is always *staged for confirmation*, never auto-dialed — the prompt
//! is the user's gate against a planted link (mirrors the in-app native arg path).
//! Best-effort throughout: any IPC failure degrades to "just run normally".

use bevy::prelude::*;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

/// Cross-platform local-socket name. Namespaced (not a filesystem path) so it
/// works identically on Linux (abstract namespace) and Windows (named pipe).
const SOCK_NAME: &str = "luncosim-deeplink.sock";

/// Shared queue of inbound deep-link URLs (initial launch arg + anything later
/// forwarded by a second process). Drained by [`drain_deep_link_inbox`].
#[derive(Resource, Clone)]
pub struct DeepLinkInbox(Arc<Mutex<VecDeque<String>>>);

/// What [`acquire`] decided this process should do.
pub enum LaunchOutcome {
    /// A running instance accepted the forwarded link; this process should exit.
    Forwarded,
    /// This process is the primary app. Insert the [`DeepLinkInbox`] resource and
    /// let [`drain_deep_link_inbox`] feed incoming links to the confirm prompt.
    Primary(DeepLinkInbox),
}

/// The `luncosim:`-scheme URL this process was launched with, if any.
fn deeplink_arg() -> Option<String> {
    let prefix = format!("{}:", crate::connect_link::SCHEME);
    std::env::args().find(|a| a.starts_with(&prefix))
}

/// Decide this process's role (see module docs). Never panics — IPC errors fall
/// back to running as a standalone primary with no cross-instance forwarding.
pub fn acquire() -> LaunchOutcome {
    use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};

    let url = deeplink_arg();

    // `Name` borrows the source string and isn't guaranteed `Clone`, so build a
    // fresh one for each of the connect / listen attempts.
    let Ok(connect_name) = SOCK_NAME.to_ns_name::<GenericNamespaced>() else {
        return primary(url);
    };

    // Only a deep-link launch forwards. A plain launch (no URL) must NEVER
    // forward-and-exit — otherwise a second window could never open on the same
    // machine (e.g. running a host + client side by side for testing). With no
    // URL we fall through to become a secondary primary: the listener bind below
    // fails (name already in use) and we degrade to a standalone instance.
    if let Some(u) = &url {
        // Is an instance already listening? If so, hand it the link and bow out.
        if let Ok(mut stream) = Stream::connect(connect_name) {
            match stream.write_all(u.as_bytes()).and_then(|()| stream.flush()) {
                Ok(()) => info!("[net] forwarded deep link to running instance: {u}"),
                Err(e) => warn!("[net] deep-link forward to running instance failed ({e})"),
            }
            return LaunchOutcome::Forwarded;
        }
    }

    // No one home — become primary and listen for future forwards.
    let Ok(listen_name) = SOCK_NAME.to_ns_name::<GenericNamespaced>() else {
        return primary(url);
    };
    let listener = match ListenerOptions::new().name(listen_name).create_sync() {
        Ok(l) => l,
        Err(e) => {
            warn!("[net] deep-link socket bind failed ({e}); single-instance off");
            return primary(url);
        }
    };

    let inbox = primary_inbox(url);
    let queue = inbox.0.clone();
    std::thread::Builder::new()
        .name("luncosim-deeplink".into())
        .spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut conn = conn;
                let mut buf = String::new();
                if conn.read_to_string(&mut buf).is_ok() {
                    let url = buf.trim();
                    if !url.is_empty() {
                        if let Ok(mut q) = queue.lock() {
                            q.push_back(url.to_string());
                        }
                    }
                }
            }
        })
        .ok();

    LaunchOutcome::Primary(inbox)
}

/// Primary outcome with no listener (IPC unavailable) — still surfaces the launch
/// arg so a first-run deep link is honored.
fn primary(url: Option<String>) -> LaunchOutcome {
    LaunchOutcome::Primary(primary_inbox(url))
}

fn primary_inbox(url: Option<String>) -> DeepLinkInbox {
    let mut q = VecDeque::new();
    if let Some(u) = url {
        q.push_back(u);
    }
    DeepLinkInbox(Arc::new(Mutex::new(q)))
}

/// Drain queued deep-link URLs into [`PendingConnect`] (confirm-gated). Each
/// parses to an address+digest; the newest wins if several arrive at once. Runs
/// only when a [`DeepLinkInbox`] was inserted (i.e. [`acquire`] ran).
pub fn drain_deep_link_inbox(
    inbox: Option<Res<DeepLinkInbox>>,
    mut pending: ResMut<lunco_core::session::PendingConnect>,
) {
    let Some(inbox) = inbox else {
        return;
    };
    let Ok(mut q) = inbox.0.lock() else {
        return;
    };
    while let Some(url) = q.pop_front() {
        if let Some(link) = crate::connect_link::parse_native(&url) {
            info!(
                "[net] deep link → pending connect to {} (awaiting confirm)",
                link.address
            );
            pending.request = Some(lunco_core::session::PendingConnectRequest {
                address: link.address,
                digest: link.digest,
            });
        }
    }
}
