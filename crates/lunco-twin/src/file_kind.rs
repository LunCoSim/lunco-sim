//! File classification — Document vs. file reference vs. unknown.
//!
//! `lunco-twin` classifies files by **extension only**. It does not open,
//! parse, or validate content. A `.mo` file is treated as a Modelica
//! Document even if its contents are gibberish — parsing is the domain
//! crate's problem, not Twin's.
//!
//! This keeps Twin fast, dependency-free of domain parsers, and resilient
//! to broken files (a user with one invalid `.mo` can still open the Twin).

use std::path::{Path, PathBuf};

/// A file discovered inside a Twin, with its classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Path of the file relative to the Twin root.
    pub relative_path: PathBuf,

    /// How this file is treated by LunCoSim.
    pub kind: FileKind,
}

/// Classification of a file inside a Twin.
///
/// Mirrors the three-way distinction in
/// [`docs/architecture/10-document-system.md`] § 2a:
///
/// - [`Document`](FileKind::Document) — editable inside LunCoSim via typed
///   ops on structured content (`.mo`, `.usda`, `.sysml`, `.mission.ron`).
/// - [`FileReference`](FileKind::FileReference) — opaque container edited
///   only in external tools (`.png`, `.glb`, `.wav`, ...). Twin tracks
///   its existence for dependency listing; no ops.
/// - [`Unknown`](FileKind::Unknown) — extension we don't recognise. Listed
///   for completeness; not shown in dedicated panels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileKind {
    /// An editable structured Document.
    Document(DocumentKind),
    /// An opaque file the Twin references but does not edit.
    FileReference,
    /// Extension not recognized by the current classifier.
    Unknown,
}

/// Which kind of Document (which domain owns the parser).
///
/// Open-ended: `Other(String)` carries an extension we *recognize as a
/// domain Document* but for which no built-in classifier entry exists
/// yet. Domain crates in the future may register additional recognizers,
/// but the baseline set here covers everything LunCoSim knows about today.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentKind {
    /// Modelica source — `.mo`.
    Modelica,
    /// USD stage — `.usda`, `.usdc`, `.usd`.
    Usd,
    /// SysML v2 source — `.sysml`.
    Sysml,
    /// Mission script — `.mission.ron`, `.mission.yaml`.
    Mission,
    /// Plain RON/YAML/TOML data Document not matching a more specific kind.
    Data,
    /// Reserved escape hatch for extensions added by domain crates in
    /// the future without breaking this enum.
    Other(String),
}

impl FileKind {
    /// Classify a path by extension. Never touches disk.
    pub fn classify(relative_path: &Path) -> FileKind {
        // Mission files have a compound extension: `*.mission.ron` or
        // `*.mission.yaml`. Check those first.
        let name = relative_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if name.ends_with(".mission.ron") || name.ends_with(".mission.yaml") {
            return FileKind::Document(DocumentKind::Mission);
        }

        let ext = relative_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        match ext.as_str() {
            // ── Document extensions ───────────────────────────────────
            "mo" => FileKind::Document(DocumentKind::Modelica),
            "usda" | "usdc" | "usd" => FileKind::Document(DocumentKind::Usd),
            "sysml" => FileKind::Document(DocumentKind::Sysml),
            "ron" | "yaml" | "yml" => FileKind::Document(DocumentKind::Data),

            // ── File references (opaque) ─────────────────────────────
            // Textures
            "png" | "jpg" | "jpeg" | "exr" | "hdr" | "tiff" | "tif" | "bmp" | "tga" | "webp" => {
                FileKind::FileReference
            }
            // Meshes
            "glb" | "gltf" | "obj" | "stl" | "fbx" | "ply" => FileKind::FileReference,
            // Audio
            "wav" | "ogg" | "mp3" | "flac" => FileKind::FileReference,
            // Docs-as-reference (today). When we author markdown/PDF
            // inside LunCoSim they may move into `Document`.
            "md" | "pdf" | "txt" | "rst" => FileKind::FileReference,
            // Video
            "mp4" | "mov" | "webm" => FileKind::FileReference,

            // ── Fallback ─────────────────────────────────────────────
            "" => FileKind::Unknown,
            _ => FileKind::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn classify(p: &str) -> FileKind {
        FileKind::classify(Path::new(p))
    }

    #[test]
    fn classifies_modelica() {
        assert_eq!(
            classify("rover.mo"),
            FileKind::Document(DocumentKind::Modelica)
        );
    }

    #[test]
    fn classifies_usd_variants() {
        assert_eq!(
            classify("scene.usda"),
            FileKind::Document(DocumentKind::Usd)
        );
        assert_eq!(
            classify("scene.usdc"),
            FileKind::Document(DocumentKind::Usd)
        );
        assert_eq!(classify("scene.usd"), FileKind::Document(DocumentKind::Usd));
    }

    #[test]
    fn classifies_sysml() {
        assert_eq!(
            classify("system.sysml"),
            FileKind::Document(DocumentKind::Sysml)
        );
    }

    #[test]
    fn classifies_mission_compound_extensions() {
        assert_eq!(
            classify("day1.mission.ron"),
            FileKind::Document(DocumentKind::Mission)
        );
        assert_eq!(
            classify("launch.mission.yaml"),
            FileKind::Document(DocumentKind::Mission)
        );
    }

    #[test]
    fn generic_ron_is_data_document() {
        // A bare `.ron` without the `.mission.` suffix is a data doc.
        assert_eq!(
            classify("config.ron"),
            FileKind::Document(DocumentKind::Data)
        );
    }

    #[test]
    fn classifies_textures_as_file_reference() {
        assert_eq!(classify("regolith.png"), FileKind::FileReference);
        assert_eq!(classify("env.exr"), FileKind::FileReference);
    }

    #[test]
    fn classifies_meshes_as_file_reference() {
        assert_eq!(classify("rover.glb"), FileKind::FileReference);
        assert_eq!(classify("part.stl"), FileKind::FileReference);
    }

    #[test]
    fn classifies_audio_and_video_as_file_reference() {
        assert_eq!(classify("click.wav"), FileKind::FileReference);
        assert_eq!(classify("demo.mp4"), FileKind::FileReference);
    }

    #[test]
    fn classifies_md_and_pdf_as_file_reference_today() {
        assert_eq!(classify("README.md"), FileKind::FileReference);
        assert_eq!(classify("spec.pdf"), FileKind::FileReference);
    }

    #[test]
    fn extension_is_case_insensitive() {
        assert_eq!(classify("MODEL.MO"), FileKind::Document(DocumentKind::Modelica));
        assert_eq!(classify("TEX.PNG"), FileKind::FileReference);
    }

    #[test]
    fn unknown_extension_is_unknown() {
        assert_eq!(classify("weird.xyz"), FileKind::Unknown);
        assert_eq!(classify("no_extension_at_all"), FileKind::Unknown);
    }

    #[test]
    fn nested_paths_classify_the_same() {
        assert_eq!(
            classify("models/subsystems/battery.mo"),
            FileKind::Document(DocumentKind::Modelica)
        );
    }
}
