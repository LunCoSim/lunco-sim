//! Bounded recents list — Twin folders + loose files.
//!
//! The Welcome page renders two columns ("Recent Twins", "Recent
//! Files") ordered most-recent first. Duplicates dedupe to the new
//! head so re-opening a project doesn't grow the list. Caps keep the
//! UI tidy and the session file small.

use std::path::PathBuf;

/// Upper bound on tracked recent Twin folders.
pub const MAX_RECENT_TWINS: usize = 10;
/// Upper bound on tracked recent loose files.
pub const MAX_RECENT_FILES: usize = 20;

/// Recently-opened items. Most-recent first.
///
/// Paths are stored as absolute `PathBuf`s; canonicalisation is a
/// consumer concern because it hits the filesystem and Workspace
/// operations want to stay pure.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Recents {
    /// Recent Twin folders. Capped at [`MAX_RECENT_TWINS`].
    pub twin_paths: Vec<PathBuf>,
    /// Recent loose files (not attached to any Twin at open time).
    /// Capped at [`MAX_RECENT_FILES`].
    pub loose_paths: Vec<PathBuf>,
}

impl Recents {
    /// Record a Twin folder as just-opened. Hoists it to the front and
    /// drops any trailing overflow.
    pub fn push_twin(&mut self, path: PathBuf) {
        push_front_dedupe(&mut self.twin_paths, path, MAX_RECENT_TWINS);
    }

    /// Record a loose file as just-opened. Same semantics as
    /// [`push_twin`](Self::push_twin).
    pub fn push_loose(&mut self, path: PathBuf) {
        push_front_dedupe(&mut self.loose_paths, path, MAX_RECENT_FILES);
    }

    /// Drop all entries. Used by "Clear Recents" actions.
    pub fn clear(&mut self) {
        self.twin_paths.clear();
        self.loose_paths.clear();
    }
}

fn push_front_dedupe(list: &mut Vec<PathBuf>, path: PathBuf, cap: usize) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    if list.len() > cap {
        list.truncate(cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_moves_existing_entry_to_front() {
        let mut r = Recents::default();
        r.push_twin("/a".into());
        r.push_twin("/b".into());
        r.push_twin("/a".into());
        assert_eq!(r.twin_paths, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
    }

    #[test]
    fn push_trims_to_cap() {
        let mut r = Recents::default();
        for i in 0..(MAX_RECENT_TWINS + 5) {
            r.push_twin(format!("/t{i}").into());
        }
        assert_eq!(r.twin_paths.len(), MAX_RECENT_TWINS);
        // Most-recent first → the last pushed path is at index 0.
        let expected_head = format!("/t{}", MAX_RECENT_TWINS + 4);
        assert_eq!(r.twin_paths[0], PathBuf::from(expected_head));
    }

    #[test]
    fn clear_empties_both_lists() {
        let mut r = Recents::default();
        r.push_twin("/x".into());
        r.push_loose("/y".into());
        r.clear();
        assert!(r.twin_paths.is_empty());
        assert!(r.loose_paths.is_empty());
    }
}
