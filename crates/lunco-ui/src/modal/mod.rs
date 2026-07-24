//! Single-host modal queue.
//!
//! Panels never call `egui::Window::show` for dialogs. They push a
//! [`ModalRequest`] to the [`ModalQueue`] resource; the host system
//! (added by [`crate::LuncoUiPlugin`]) renders one modal at a time on
//! top of every panel using `egui::Modal`, which gives us scrim,
//! pointer-event blocking, and Esc-dismiss for free.
//!
//! The owner of a [`ModalId`] polls [`ModalQueue::poll`] each frame
//! until it returns an outcome, then dispatches the appropriate typed
//! command. The queue does not store outcomes indefinitely — once
//! polled, the result is consumed.

pub mod host;

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui;

/// Opaque id for a queued modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModalId(u64);

/// What the user did. The host always produces exactly one outcome
/// per [`ModalRequest`] before retiring it from the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalOutcome {
    /// User pressed the confirm-kind button (or the only button).
    /// Carries the button label for multi-confirm dialogs.
    Confirmed(String),
    /// User pressed a cancel-kind button or dismissed via Esc / scrim.
    Cancelled,
    /// User pressed a destructive-kind button. Distinct from
    /// `Confirmed` so callers can require an extra check.
    Destructive(String),
}

/// Button kinds carry semantic meaning so the host can paint them
/// consistently and so Esc-dismiss can map to the right outcome.
pub enum ModalButton {
    /// Primary action (e.g. "Save", "Compile"). Mapped from Enter.
    Confirm(String),
    /// Cancel / no-op. Mapped from Esc when `dismiss_on_esc` is set.
    Cancel(String),
    /// Destructive action (e.g. "Discard changes"). Painted as warning.
    Destructive(String),
}

/// Body of a modal. Either a static message string or a custom
/// closure that paints into the modal's `Ui`. The closure must be
/// `Send + Sync` because it lives on a Bevy resource.
pub enum ModalBody {
    Text(String),
    Custom(Arc<dyn Fn(&mut egui::Ui) + Send + Sync>),
}

/// One pending modal.
pub struct ModalRequest {
    pub title: String,
    pub body: ModalBody,
    pub buttons: Vec<ModalButton>,
    /// When `true`, Esc closes the modal with [`ModalOutcome::Cancelled`].
    pub dismiss_on_esc: bool,
}

/// Workbench-wide modal queue. Owned by [`crate::LuncoUiPlugin`].
#[derive(Resource, Default)]
pub struct ModalQueue {
    next_id: u64,
    pub(crate) pending: Vec<(ModalId, ModalRequest)>,
    pub(crate) results: HashMap<ModalId, ModalOutcome>,
}

impl ModalQueue {
    /// Enqueue `request` for display. Returns the id used by
    /// [`Self::poll`] to retrieve the outcome.
    pub fn request(&mut self, request: ModalRequest) -> ModalId {
        let id = ModalId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        self.pending.push((id, request));
        id
    }

    /// Returns `Some(outcome)` exactly once after the user resolves
    /// the modal `id`, then forgets the result.
    pub fn poll(&mut self, id: ModalId) -> Option<ModalOutcome> {
        self.results.remove(&id)
    }

    /// Withdraw a modal request whose owner has lost interest — e.g.
    /// the document the dialog targeted was closed via another path
    /// before the user acted on it. Removes the request from the
    /// pending queue *and* any unpolled outcome from `results`, so it
    /// can never surface as a ghost dialog after its context is gone.
    /// No-op when `id` was already polled or never existed.
    pub fn cancel(&mut self, id: ModalId) {
        self.pending.retain(|(qid, _)| *qid != id);
        self.results.remove(&id);
    }

    /// `true` if any modal is currently displayed or queued.
    pub fn is_active(&self) -> bool {
        !self.pending.is_empty() || !self.results.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(title: &str) -> ModalRequest {
        ModalRequest {
            title: title.to_string(),
            body: ModalBody::Text(String::new()),
            buttons: vec![ModalButton::Confirm("OK".into())],
            dismiss_on_esc: true,
        }
    }

    #[test]
    fn request_returns_unique_ids() {
        let mut q = ModalQueue::default();
        let a = q.request(req("A"));
        let b = q.request(req("B"));
        assert_ne!(a, b);
        assert_eq!(q.pending.len(), 2);
        assert!(q.is_active());
    }

    #[test]
    fn poll_consumes_outcome_exactly_once() {
        let mut q = ModalQueue::default();
        let id = q.request(req("A"));
        // Simulate the host resolving the modal.
        q.results.insert(id, ModalOutcome::Confirmed("OK".into()));
        assert_eq!(q.poll(id), Some(ModalOutcome::Confirmed("OK".into())));
        assert_eq!(q.poll(id), None);
    }

    #[test]
    fn cancel_removes_pending_request() {
        let mut q = ModalQueue::default();
        let a = q.request(req("A"));
        let b = q.request(req("B"));
        q.cancel(a);
        assert_eq!(q.pending.len(), 1);
        assert_eq!(q.pending[0].0, b);
    }

    #[test]
    fn cancel_drops_unpolled_outcome() {
        let mut q = ModalQueue::default();
        let id = q.request(req("A"));
        q.results.insert(id, ModalOutcome::Confirmed("OK".into()));
        q.cancel(id);
        assert_eq!(q.poll(id), None);
    }

    #[test]
    fn cancel_unknown_id_is_noop() {
        let mut q = ModalQueue::default();
        let id = q.request(req("A"));
        q.cancel(ModalId(9999));
        assert_eq!(q.pending.len(), 1);
        // Original entry untouched.
        assert_eq!(q.pending[0].0, id);
    }

    #[test]
    fn pending_order_preserved() {
        let mut q = ModalQueue::default();
        let ids: Vec<_> = ["A", "B", "C"].iter().map(|t| q.request(req(t))).collect();
        let order: Vec<_> = q.pending.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, ids);
    }
}
