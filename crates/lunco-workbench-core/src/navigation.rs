//! Domain-owned navigation entries consumed by the workbench's quick-open UI.

use std::sync::Arc;

use bevy::prelude::{Resource, World};

use crate::MenuCtx;

/// Exact source and presentation purpose supplied by an editor instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavigationSource {
    pub uri: String,
    pub mode: String,
}

/// A discoverable destination. Its owner emits the existing typed operation.
#[derive(Clone)]
pub struct NavigationEntry {
    /// Stable, owner-qualified identity; labels never determine routing.
    pub id: String,
    pub label: String,
    pub detail: String,
    pub category: String,
    pub activate: Arc<dyn Fn(&mut MenuCtx) + Send + Sync>,
}

impl NavigationEntry {
    /// Match every query word against readable labels and source paths.
    pub fn matches(&self, query: &str) -> bool {
        let text = format!("{} {} {}", self.label, self.detail, self.category).to_lowercase();
        query
            .split_whitespace()
            .all(|word| text.contains(&word.to_lowercase()))
    }
}

type NavigationProvider = Arc<dyn Fn(&World) -> Result<Vec<NavigationEntry>, String> + Send + Sync>;

pub struct NavigationInventory {
    pub entries: Vec<NavigationEntry>,
    pub errors: Vec<String>,
}

/// Providers read their existing inventories; this registry owns no asset index.
#[derive(Resource, Default)]
pub struct NavigationRegistry {
    providers: Vec<(Vec<String>, NavigationProvider)>,
}

impl NavigationRegistry {
    pub fn register(
        &mut self,
        categories: &[&str],
        provider: impl Fn(&World) -> Result<Vec<NavigationEntry>, String> + Send + Sync + 'static,
    ) {
        self.providers.push((
            categories
                .iter()
                .map(|category| (*category).to_owned())
                .collect(),
            Arc::new(provider),
        ));
    }

    /// Called on explicit search requests, never during panel painting.
    pub fn entries(&self, world: &World, category: &str) -> NavigationInventory {
        let mut inventory = NavigationInventory {
            entries: Vec::new(),
            errors: Vec::new(),
        };
        for (categories, provider) in &self.providers {
            if !category.is_empty() && !categories.iter().any(|candidate| candidate == category) {
                continue;
            }
            match provider(world) {
                Ok(entries) => inventory.entries.extend(entries),
                Err(error) => inventory.errors.push(error),
            }
        }
        inventory
    }
}
