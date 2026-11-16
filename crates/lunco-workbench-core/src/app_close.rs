//! Application close admission before document and renderer teardown.
//! Document registries retain ownership of the ordinary dirty-document flow.

use bevy::prelude::{Resource, World};
use std::collections::BTreeMap;

/// A live editor owner can reveal the unapplied work that prevents closing.
pub struct AppCloseBlocker {
    pub message: String,
    pub reveal: fn(&mut World),
}

type Check = fn(&World) -> Option<AppCloseBlocker>;

/// Application-scoped checks read current owner resources at close admission.
/// Registered functions capture no scene data and survive Twin replacement.
#[derive(Resource, Default)]
pub struct AppCloseGuards(BTreeMap<&'static str, Check>);

impl AppCloseGuards {
    pub fn register(&mut self, owner: &'static str, check: fn(&World) -> Option<AppCloseBlocker>) {
        assert!(
            !self.0.contains_key(owner),
            "app-close guard already registered: {owner}"
        );
        self.0.insert(owner, check);
    }

    pub fn check(&self, world: &World) -> Option<AppCloseBlocker> {
        self.0.values().find_map(|check| check(world))
    }
}
