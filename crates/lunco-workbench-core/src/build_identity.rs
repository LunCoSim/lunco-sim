use bevy::prelude::Resource;

/// Product version and source identity supplied by the host application.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Release or product version shown to users.
    pub version: String,
    /// Build identifier, normally the short source revision.
    pub build: String,
    /// Canonical GitHub repository containing the source revision.
    pub repository: String,
}

impl BuildIdentity {
    /// Create an identity from the host application's stamped values.
    pub fn new(
        version: impl Into<String>,
        build: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            version: version.into(),
            build: build.into(),
            repository: repository.into(),
        }
    }

    /// Format the canonical version line shared by Help and Settings.
    pub fn version_label(&self) -> String {
        format!("Version {} ({})", self.version, self.build)
    }

    /// Return the exact source revision URL when the build has a known SHA.
    pub fn source_url(&self) -> Option<String> {
        let revision = self.build.strip_suffix("-dirty").unwrap_or(&self.build);
        if revision.is_empty() || revision == "unknown" {
            return None;
        }
        Some(format!(
            "{}/commit/{}",
            self.repository.trim_end_matches('/'),
            revision
        ))
    }
}
