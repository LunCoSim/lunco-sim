//! Tab table and lifecycle logic for Modelica model views.

use std::collections::{HashMap, HashSet};
use bevy::prelude::*;
use lunco_doc::DocumentId;
use crate::model_tabs_types::{ModelTabState, ModelViewMode, TabId};

/// Registry of open model view tabs.
#[derive(Resource, Default)]
pub struct ModelTabs {
    pub(super) tabs: HashMap<TabId, ModelTabState>,
    next_id: u64,
    preview_slot: Option<TabId>,
}

impl ModelTabs {
    fn allocate_id(&mut self) -> TabId {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    /// The tab id of this doc's primary (non-drilled) tab if open, else
    /// any tab on the doc, else `None`. Read-only — used by session
    /// hot-exit to record / re-resolve the dock tab instance for a doc
    /// without mutating the table.
    pub fn primary_tab_for(&self, doc: DocumentId) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|(_, s)| s.doc == doc && s.drilled_class.is_none())
            .or_else(|| self.tabs.iter().find(|(_, s)| s.doc == doc))
            .map(|(id, _)| *id)
    }

    pub fn ensure_for(
        &mut self,
        doc: DocumentId,
        drilled_class: Option<String>,
    ) -> TabId {
        if let Some((id, _)) = self.tabs.iter().find(|(_, s)| {
            s.doc == doc && s.drilled_class.as_deref() == drilled_class.as_deref()
        }) {
            return *id;
        }
        let id = self.allocate_id();
        self.tabs.insert(
            id,
            ModelTabState {
                doc,
                drilled_class,
                view_mode: ModelViewMode::default(),
                pinned: true,
            },
        );
        id
    }

    /// Rewrite every `drilled_class == Some(old)` tab on `doc` to
    /// `Some(new)`. Called by the `ClassRenamed` change observer so
    /// tabs follow class identity through a rename instead of going
    /// stale and forcing a duplicate-tab.
    pub fn rename_drilled_class(
        &mut self,
        doc: DocumentId,
        old: &str,
        new: &str,
    ) -> usize {
        let mut hit = 0;
        for state in self.tabs.values_mut() {
            if state.doc == doc && state.drilled_class.as_deref() == Some(old) {
                state.drilled_class = Some(new.to_string());
                hit += 1;
            }
        }
        hit
    }

    pub fn ensure_preview_for(
        &mut self,
        doc: DocumentId,
        drilled_class: Option<String>,
    ) -> (TabId, Option<TabId>) {
        self.ensure_preview_for_with_default(doc, drilled_class, None)
    }

    /// Variant of [`Self::ensure_preview_for`] that accepts the doc's
    /// *default class* (the first non-package top-level class).
    /// Treats `drilled_class = None` and `drilled_class =
    /// Some(default)` as the same view, so a Twin Browser click on
    /// the default class focuses the existing file tab (which was
    /// allocated with `None`) and vice-versa. Multi-class files
    /// keep distinct drill semantics for any non-default class.
    pub fn ensure_preview_for_with_default(
        &mut self,
        doc: DocumentId,
        drilled_class: Option<String>,
        default_class: Option<&str>,
    ) -> (TabId, Option<TabId>) {
        let target_is_default = match (drilled_class.as_deref(), default_class) {
            (None, _) => true,
            (Some(d), Some(def)) => d == def,
            _ => false,
        };
        if let Some((id, _)) = self.tabs.iter().find(|(_, s)| {
            if s.doc != doc {
                return false;
            }
            if s.drilled_class.as_deref() == drilled_class.as_deref() {
                return true;
            }
            if !target_is_default {
                return false;
            }
            // Either side of the (None, Some(default)) pair matches
            // the file-tab view.
            match s.drilled_class.as_deref() {
                None => true,
                Some(s_class) => default_class
                    .is_some_and(|def| s_class == def),
            }
        }) {
            return (*id, None);
        }
        let id = self.allocate_id();
        self.tabs.insert(
            id,
            ModelTabState {
                doc,
                drilled_class,
                view_mode: ModelViewMode::default(),
                pinned: false,
            },
        );
        let evict = self.preview_slot.replace(id);
        (id, evict)
    }

    pub fn open_new(
        &mut self,
        doc: DocumentId,
        drilled_class: Option<String>,
    ) -> TabId {
        let id = self.allocate_id();
        self.tabs.insert(
            id,
            ModelTabState {
                doc,
                drilled_class,
                view_mode: ModelViewMode::default(),
                pinned: true,
            },
        );
        id
    }

    /// Force a tab's view mode. Used at open-time to default
    /// non-Modelica files to [`ModelViewMode::Text`] (no point rendering
    /// an empty Canvas for content that has no classes / connectors).
    pub fn set_view_mode(&mut self, tab_id: TabId, mode: ModelViewMode) {
        if let Some(state) = self.tabs.get_mut(&tab_id) {
            state.view_mode = mode;
        }
    }

    pub fn pin(&mut self, tab_id: TabId) {
        if let Some(state) = self.tabs.get_mut(&tab_id) {
            state.pinned = true;
        }
        if self.preview_slot == Some(tab_id) {
            self.preview_slot = None;
        }
    }

    pub fn pin_all_for_doc(&mut self, doc: DocumentId) {
        let mut clear_preview = false;
        for (id, state) in self.tabs.iter_mut() {
            if state.doc == doc {
                state.pinned = true;
                if self.preview_slot == Some(*id) {
                    clear_preview = true;
                }
            }
        }
        if clear_preview {
            self.preview_slot = None;
        }
    }

    pub fn close_tab(&mut self, tab_id: TabId) -> Option<ModelTabState> {
        if self.preview_slot == Some(tab_id) {
            self.preview_slot = None;
        }
        self.tabs.remove(&tab_id)
    }

    pub fn iter_mut_for_doc(
        &mut self,
        doc: DocumentId,
    ) -> impl Iterator<Item = (TabId, &mut ModelTabState)> + '_ {
        self.tabs
            .iter_mut()
            .filter(move |(_, s)| s.doc == doc)
            .map(|(id, s)| (*id, s))
    }

    pub fn drilled_class_for_doc(&self, doc: DocumentId) -> Option<String> {
        let tab_id = self.any_for_doc(doc)?;
        self.get(tab_id)?.drilled_class.clone()
    }

    pub fn close_drilled_into(&mut self, doc: DocumentId, qualified: &str) -> Vec<TabId> {
        if qualified.is_empty() {
            return Vec::new();
        }
        let prefix = format!("{qualified}.");
        let to_close: Vec<TabId> = self
            .tabs
            .iter()
            .filter_map(|(id, s)| {
                if s.doc != doc {
                    return None;
                }
                let drilled = s.drilled_class.as_deref()?;
                (drilled == qualified || drilled.starts_with(&prefix)).then_some(*id)
            })
            .collect();
        for id in &to_close {
            self.tabs.remove(id);
        }
        to_close
    }

    pub fn close_all_for_doc(&mut self, doc: DocumentId) -> Vec<TabId> {
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter_map(|(id, s)| (s.doc == doc).then_some(*id))
            .collect();
        for id in &ids {
            self.tabs.remove(id);
        }
        ids
    }

    pub fn close(&mut self, doc: DocumentId) {
        let _ = self.close_all_for_doc(doc);
    }

    pub fn get(&self, tab_id: TabId) -> Option<&ModelTabState> {
        self.tabs.get(&tab_id)
    }

    pub fn get_mut(&mut self, tab_id: TabId) -> Option<&mut ModelTabState> {
        self.tabs.get_mut(&tab_id)
    }

    pub fn any_for_doc(&self, doc: DocumentId) -> Option<TabId> {
        self.tabs
            .iter()
            .find_map(|(id, s)| (s.doc == doc).then_some(*id))
    }

    pub fn find_for(
        &self,
        doc: DocumentId,
        drilled_class: Option<&str>,
    ) -> Option<TabId> {
        self.tabs.iter().find_map(|(id, s)| {
            (s.doc == doc && s.drilled_class.as_deref() == drilled_class).then_some(*id)
        })
    }

    pub fn find_for_mut(
        &mut self,
        doc: DocumentId,
        drilled_class: Option<&str>,
    ) -> Option<&mut ModelTabState> {
        self.tabs.iter_mut().find_map(|(_, s)| {
            (s.doc == doc && s.drilled_class.as_deref() == drilled_class).then_some(s)
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = (TabId, &ModelTabState)> + '_ {
        self.tabs.iter().map(|(id, s)| (*id, s))
    }

    pub fn iter_docs(&self) -> impl Iterator<Item = DocumentId> + '_ {
        let mut seen = HashSet::new();
        self.tabs
            .values()
            .filter_map(move |s| seen.insert(s.doc).then_some(s.doc))
    }

    pub fn contains(&self, doc: DocumentId) -> bool {
        self.any_for_doc(doc).is_some()
    }

    pub fn count_for_doc(&self, doc: DocumentId) -> usize {
        self.tabs.values().filter(|s| s.doc == doc).count()
    }
}
