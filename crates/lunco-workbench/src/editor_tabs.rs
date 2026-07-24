//! Domain-neutral lifecycle for document editor tab instances.

use std::collections::HashMap;

use bevy::prelude::Resource;

/// Workbench-local identity of an editor tab instance.
pub type EditorTabId = u64;

/// One editor tab: shared lifecycle metadata plus domain-owned state.
#[derive(Debug, Clone)]
pub struct EditorTab<T> {
    /// State interpreted by the editor domain.
    pub state: T,
    /// Whether this tab survives opening another preview.
    pub pinned: bool,
}

/// Reusable preview/pin lifecycle for an editor family.
#[derive(Resource, Debug)]
pub struct EditorTabs<T: Send + Sync + 'static> {
    tabs: HashMap<EditorTabId, EditorTab<T>>,
    next_id: EditorTabId,
    preview: Option<EditorTabId>,
}

impl<T: Send + Sync + 'static> Default for EditorTabs<T> {
    fn default() -> Self {
        Self {
            tabs: HashMap::new(),
            next_id: 0,
            preview: None,
        }
    }
}

impl<T: Send + Sync + 'static> EditorTabs<T> {
    fn allocate(&mut self) -> EditorTabId {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    /// Find a tab whose domain state matches a view identity.
    pub fn find(&self, matches: impl Fn(&T) -> bool) -> Option<EditorTabId> {
        self.tabs
            .iter()
            .find_map(|(id, tab)| matches(&tab.state).then_some(*id))
    }

    /// Focus an equivalent tab or create a pinned one.
    pub fn ensure_pinned(
        &mut self,
        matches: impl Fn(&T) -> bool,
        state: impl FnOnce() -> T,
    ) -> EditorTabId {
        if let Some(id) = self.find(matches) {
            return id;
        }
        self.open_pinned(state())
    }

    /// Focus an equivalent tab or replace the editor family's preview slot.
    ///
    /// The returned eviction id remains in this table until the domain closes
    /// its dock tab and calls [`Self::close`].
    pub fn ensure_preview(
        &mut self,
        matches: impl Fn(&T) -> bool,
        state: impl FnOnce() -> T,
    ) -> (EditorTabId, Option<EditorTabId>) {
        if let Some(id) = self.find(matches) {
            return (id, None);
        }
        let id = self.allocate();
        self.tabs.insert(
            id,
            EditorTab {
                state: state(),
                pinned: false,
            },
        );
        let evicted = self.preview.replace(id);
        (id, evicted)
    }

    /// Open a deliberate duplicate that is pinned from creation.
    pub fn open_pinned(&mut self, state: T) -> EditorTabId {
        let id = self.allocate();
        self.tabs.insert(
            id,
            EditorTab {
                state,
                pinned: true,
            },
        );
        id
    }

    /// Promote a preview into a durable tab.
    pub fn pin(&mut self, id: EditorTabId) -> bool {
        let Some(tab) = self.tabs.get_mut(&id) else {
            return false;
        };
        tab.pinned = true;
        if self.preview == Some(id) {
            self.preview = None;
        }
        true
    }

    /// Remove a tab and return its domain state.
    pub fn close(&mut self, id: EditorTabId) -> Option<T> {
        if self.preview == Some(id) {
            self.preview = None;
        }
        self.tabs.remove(&id).map(|tab| tab.state)
    }

    /// Read one tab.
    pub fn get(&self, id: EditorTabId) -> Option<&EditorTab<T>> {
        self.tabs.get(&id)
    }

    /// Mutate one tab.
    pub fn get_mut(&mut self, id: EditorTabId) -> Option<&mut EditorTab<T>> {
        self.tabs.get_mut(&id)
    }

    /// Iterate all tabs.
    pub fn iter(&self) -> impl Iterator<Item = (EditorTabId, &EditorTab<T>)> {
        self.tabs.iter().map(|(id, tab)| (*id, tab))
    }

    /// Mutably iterate all tabs.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EditorTabId, &mut EditorTab<T>)> {
        self.tabs.iter_mut().map(|(id, tab)| (*id, tab))
    }

    /// Current unpinned preview, if any.
    pub fn preview(&self) -> Option<EditorTabId> {
        self.preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_preview_evicts_only_the_previous_preview() {
        let mut tabs = EditorTabs::default();
        let pinned = tabs.open_pinned("pinned");
        let (first, evicted) = tabs.ensure_preview(|_| false, || "first");
        assert_eq!(evicted, None);

        let (second, evicted) = tabs.ensure_preview(|_| false, || "second");
        assert_eq!(evicted, Some(first));
        assert!(tabs.get(pinned).is_some());
        assert!(
            tabs.get(first).is_some(),
            "caller closes the returned dock tab"
        );
        assert_eq!(tabs.preview(), Some(second));
    }

    #[test]
    fn pinning_a_preview_clears_the_preview_slot() {
        let mut tabs = EditorTabs::default();
        let (id, _) = tabs.ensure_preview(|_| false, || "file");
        assert!(tabs.pin(id));
        assert!(tabs.get(id).is_some_and(|tab| tab.pinned));
        assert_eq!(tabs.preview(), None);
    }

    #[test]
    fn ensure_focuses_an_equivalent_existing_view() {
        let mut tabs = EditorTabs::default();
        let first = tabs.ensure_pinned(|value| *value == "same", || "same");
        let second = tabs.ensure_pinned(|value| *value == "same", || "same");
        assert_eq!(first, second);
        assert_eq!(tabs.iter().count(), 1);
    }
}
