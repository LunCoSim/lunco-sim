//! Shared path-to-directory-tree projection for browser sections.

use std::{collections::BTreeMap, path::Path};

/// One directory in a browser tree.
pub(super) struct PathTree<T> {
    /// Leaves directly inside this directory.
    pub(super) files: Vec<T>,
    /// Child directories in stable display order.
    pub(super) subdirs: BTreeMap<String, PathTree<T>>,
}

impl<T> Default for PathTree<T> {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            subdirs: BTreeMap::new(),
        }
    }
}

/// Build a directory tree from `(relative path, leaf)` pairs.
pub(super) fn build_path_tree<P: AsRef<Path>, T>(
    entries: impl IntoIterator<Item = (P, T)>,
) -> PathTree<T> {
    let mut root = PathTree::default();
    for (path, leaf) in entries {
        let path = path.as_ref();
        let Some(parent) = path.parent() else {
            root.files.push(leaf);
            continue;
        };
        let mut current = &mut root;
        for component in parent.components() {
            let std::path::Component::Normal(name) = component else {
                continue;
            };
            current = current
                .subdirs
                .entry(name.to_string_lossy().into_owned())
                .or_default();
        }
        current.files.push(leaf);
    }
    root
}
