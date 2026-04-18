//! Selection state — which nodes and edges the user has picked.
//!
//! Kept out of [`crate::scene::Scene`] deliberately: serialising and
//! diffing a scene for undo/redo should not churn on highlight
//! changes. The selection is also per-view — two panels looking at
//! the same scene can (eventually) have independent selections.
//!
//! # Primary vs set
//!
//! Figma/Dymola both distinguish the *set* of selected items from
//! the single *primary* one (the one the Inspector shows properties
//! for). `Selection` stores both: `primary` is always a member of
//! `items` when non-empty; it's the last-added or last-clicked item.

use std::collections::BTreeSet;

use crate::scene::{EdgeId, NodeId};

/// Anything that can be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SelectItem {
    Node(NodeId),
    Edge(EdgeId),
}

/// The user's current selection.
///
/// `items` is a sorted set for stable iteration (used by the
/// Inspector, by "align selected", etc.); `primary` is the focus.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    items: BTreeSet<SelectItem>,
    primary: Option<SelectItem>,
}

impl Selection {
    /// Clear completely.
    pub fn clear(&mut self) {
        self.items.clear();
        self.primary = None;
    }

    /// Replace with a single item (click-to-select behaviour).
    pub fn set(&mut self, item: SelectItem) {
        self.items.clear();
        self.items.insert(item);
        self.primary = Some(item);
    }

    /// Add `item` to the current selection (shift-click extend).
    /// If already present, this is a no-op except the primary is
    /// updated — matching the "click to focus" behaviour users
    /// expect from explorer-style multi-select.
    pub fn add(&mut self, item: SelectItem) {
        self.items.insert(item);
        self.primary = Some(item);
    }

    /// Remove `item` if present. If it was the primary, the primary
    /// falls back to the first remaining item (or `None` if the set
    /// is now empty) so the Inspector always has a target when any
    /// selection exists.
    pub fn remove(&mut self, item: SelectItem) -> bool {
        let was_present = self.items.remove(&item);
        if self.primary == Some(item) {
            self.primary = self.items.iter().next().copied();
        }
        was_present
    }

    /// Toggle: add if absent, remove if present. ctrl-click behaviour.
    pub fn toggle(&mut self, item: SelectItem) {
        if self.items.contains(&item) {
            self.remove(item);
        } else {
            self.add(item);
        }
    }

    pub fn contains(&self, item: SelectItem) -> bool {
        self.items.contains(&item)
    }
    pub fn primary(&self) -> Option<SelectItem> {
        self.primary
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn iter(&self) -> impl Iterator<Item = &SelectItem> {
        self.items.iter()
    }

    /// Convenience: just the nodes. Used by "move selection" drag.
    pub fn nodes(&self) -> BTreeSet<NodeId> {
        self.items
            .iter()
            .filter_map(|it| match it {
                SelectItem::Node(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Convenience: just the edges.
    pub fn edges(&self) -> BTreeSet<EdgeId> {
        self.items
            .iter()
            .filter_map(|it| match it {
                SelectItem::Edge(id) => Some(*id),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_previous_selection() {
        let mut s = Selection::default();
        s.set(SelectItem::Node(NodeId(1)));
        s.set(SelectItem::Node(NodeId(2)));
        assert_eq!(s.len(), 1);
        assert_eq!(s.primary(), Some(SelectItem::Node(NodeId(2))));
    }

    #[test]
    fn toggle_flips_membership_and_primary_follows() {
        let mut s = Selection::default();
        let a = SelectItem::Node(NodeId(1));
        let b = SelectItem::Node(NodeId(2));
        s.toggle(a);
        s.toggle(b);
        assert_eq!(s.len(), 2);
        assert_eq!(s.primary(), Some(b)); // most recently added
        s.toggle(b);
        assert_eq!(s.len(), 1);
        // Primary fell back to the remaining item.
        assert_eq!(s.primary(), Some(a));
    }

    #[test]
    fn removing_primary_picks_a_new_one() {
        let mut s = Selection::default();
        s.add(SelectItem::Node(NodeId(10)));
        s.add(SelectItem::Node(NodeId(20)));
        // Primary = 20, remove it.
        s.remove(SelectItem::Node(NodeId(20)));
        assert_eq!(s.primary(), Some(SelectItem::Node(NodeId(10))));
    }

    #[test]
    fn removing_last_clears_primary() {
        let mut s = Selection::default();
        s.add(SelectItem::Edge(EdgeId(5)));
        s.remove(SelectItem::Edge(EdgeId(5)));
        assert!(s.is_empty());
        assert_eq!(s.primary(), None);
    }
}
