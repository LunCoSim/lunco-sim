//! Headless Modelica compiler and source-admission host.
//!
//! This package owns the long-lived Rumoca session used by workers, runners,
//! asset tooling, and command-line hosts. It deliberately has no Bevy plugin:
//! the UI/document lifecycle belongs to `lunco-modelica-core`, while this
//! package is the shared production compiler used by both runtime and tests.

use rumoca_compile::{Session, SessionConfig};

mod interface;

#[cfg(not(target_arch = "wasm32"))]
mod compiler_heartbeat;

#[cfg(test)]
mod interface_tests;

const SOURCE_SET_REVISION_VERSION: u32 = 1;

fn source_content_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
    bytes.extend_from_slice(value);
}

/// Strong content identity for one exact Modelica source file in an admitted
/// source root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelicaSourceFileContent {
    /// Portable logical URI within the source root.
    pub source_uri: String,
    /// CIDv1 raw/SHA-256 identity for the exact UTF-8 source bytes.
    pub cid: lunco_hash::content::Cid,
}

/// Exact source files represented by one successfully admitted source root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelicaSourceRootContent {
    /// Stable source-root identity supplied by the admission owner.
    pub source_set_id: String,
    /// Sources sorted by portable logical URI.
    pub files: Vec<ModelicaSourceFileContent>,
}

/// Why an admitted Modelica source root cannot provide an exact portable
/// content snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelicaSourceRootContentError {
    /// Source preparation retained diagnostics and therefore has no complete set.
    SourceSetHasDiagnostics,
    /// The source-root owner did not provide a logical identity.
    EmptySourceSetId,
    /// The source-root identity is not a portable logical identifier.
    NonPortableSourceSetId { source_set_id: String },
    /// A file did not have a logical URI.
    EmptyFileUri,
    /// A file URI could not be made portable relative to its source root.
    NonPortableFileUri { source_uri: String },
    /// Multiple source bodies claim the same logical URI.
    DuplicateFileUri { source_uri: String },
    /// The latest source-root install did not commit.
    SourceRootAdmissionFailed {
        source_set_id: String,
        details: String,
    },
}

impl std::fmt::Display for ModelicaSourceRootContentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceSetHasDiagnostics => {
                formatter.write_str("Modelica source root has preparation diagnostics")
            }
            Self::EmptySourceSetId => {
                formatter.write_str("Modelica source root has an empty identity")
            }
            Self::NonPortableSourceSetId { source_set_id } => write!(
                formatter,
                "Modelica source-root identity `{source_set_id}` is not portable"
            ),
            Self::EmptyFileUri => formatter.write_str("Modelica source file has an empty URI"),
            Self::NonPortableFileUri { source_uri } => write!(
                formatter,
                "Modelica source URI `{source_uri}` is not portable within its source root"
            ),
            Self::DuplicateFileUri { source_uri } => write!(
                formatter,
                "Modelica source root duplicates file URI `{source_uri}`"
            ),
            Self::SourceRootAdmissionFailed {
                source_set_id,
                details,
            } => write!(
                formatter,
                "Modelica source root `{source_set_id}` failed admission: {details}"
            ),
        }
    }
}

impl std::error::Error for ModelicaSourceRootContentError {}

use lunco_assets_path::uri_scheme;

fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_file_uri(value: &str) -> bool {
    uri_scheme(value).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file"))
}

fn is_nonportable_source_set_id(value: &str) -> bool {
    use std::path::{Component, Path};

    if is_file_uri(value)
        || Path::new(value).is_absolute()
        || value.starts_with('/')
        || value.starts_with('\\')
        || has_windows_drive_prefix(value)
    {
        return true;
    }
    if uri_scheme(value).is_some() {
        return false;
    }
    Path::new(&value.replace('\\', "/"))
        .components()
        .any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
}

fn relative_path_uri(
    path: &std::path::Path,
    source_uri: &str,
) -> Result<String, ModelicaSourceRootContentError> {
    use std::path::Component;

    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => components.push(segment.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ModelicaSourceRootContentError::NonPortableFileUri {
                    source_uri: source_uri.to_owned(),
                });
            }
        }
    }
    let uri = components.join("/");
    if uri.is_empty() {
        return Err(ModelicaSourceRootContentError::EmptyFileUri);
    }
    Ok(uri)
}

fn source_root_content_closure(
    id: &str,
    label: &str,
    source_files: &[(String, String)],
    diagnostics: &[String],
) -> Result<ModelicaSourceRootContent, ModelicaSourceRootContentError> {
    use std::path::Path;

    if !diagnostics.is_empty() {
        return Err(ModelicaSourceRootContentError::SourceSetHasDiagnostics);
    }
    if id.is_empty() {
        return Err(ModelicaSourceRootContentError::EmptySourceSetId);
    }
    if is_nonportable_source_set_id(id) {
        return Err(ModelicaSourceRootContentError::NonPortableSourceSetId {
            source_set_id: id.to_owned(),
        });
    }

    let mut files = Vec::with_capacity(source_files.len());
    for (source_uri, source) in source_files {
        if source_uri.is_empty() {
            return Err(ModelicaSourceRootContentError::EmptyFileUri);
        }
        if is_file_uri(source_uri) {
            return Err(ModelicaSourceRootContentError::NonPortableFileUri {
                source_uri: source_uri.clone(),
            });
        }
        let path = Path::new(source_uri);
        let logical_uri = if path.is_absolute() {
            let relative = path.strip_prefix(Path::new(label)).map_err(|_| {
                ModelicaSourceRootContentError::NonPortableFileUri {
                    source_uri: source_uri.clone(),
                }
            })?;
            relative_path_uri(relative, source_uri)?
        } else if has_windows_drive_prefix(source_uri)
            || source_uri.starts_with("\\\\")
            || source_uri.starts_with("//")
        {
            return Err(ModelicaSourceRootContentError::NonPortableFileUri {
                source_uri: source_uri.clone(),
            });
        } else if uri_scheme(source_uri).is_some() {
            source_uri.clone()
        } else {
            let normalized_path = source_uri.replace('\\', "/");
            relative_path_uri(Path::new(&normalized_path), source_uri)?
        };
        files.push(ModelicaSourceFileContent {
            source_uri: logical_uri,
            cid: lunco_hash::content::cid(source.as_bytes()),
        });
    }
    files.sort_unstable_by(|left, right| left.source_uri.cmp(&right.source_uri));
    for pair in files.windows(2) {
        if pair[0].source_uri == pair[1].source_uri {
            return Err(ModelicaSourceRootContentError::DuplicateFileUri {
                source_uri: pair[0].source_uri.clone(),
            });
        }
    }
    Ok(ModelicaSourceRootContent {
        source_set_id: id.to_owned(),
        files,
    })
}

fn source_set_revision(id: &str, files: &[(String, String)]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut entries = files
        .iter()
        .map(|(uri, source)| (uri.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    entries.sort_unstable();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    SOURCE_SET_REVISION_VERSION.hash(&mut hasher);
    id.hash(&mut hasher);
    for (uri, source) in entries {
        uri.hash(&mut hasher);
        source.hash(&mut hasher);
    }
    hasher.finish()
}

fn source_roots_from_parsed_docs(
    docs: &[rumoca_compile::compile::ParsedSourceDocument],
) -> std::collections::HashSet<String> {
    docs.iter()
        .flat_map(|document| document.definition().declared_class_names())
        .filter_map(|name| {
            rumoca_core::qualified_name_segments(&name)
                .next()
                .map(str::to_owned)
        })
        .collect()
}

/// Immutable result of preparing a source root before it is installed into the
/// long-lived Rumoca session. File reads, bound-input extraction, and parsing
/// can run on a preparation worker; only [`ModelicaCompiler::install_source_root`]
/// mutates the compiler session.
pub struct PreparedSourceRoot {
    id: String,
    label: String,
    files: Vec<(String, String)>,
    parsed: Vec<rumoca_compile::compile::ParsedSourceDocument>,
    parsed_roots: std::collections::HashSet<String>,
    diagnostics: Vec<String>,
    content_closure: Result<ModelicaSourceRootContent, ModelicaSourceRootContentError>,
}

impl PreparedSourceRoot {
    /// Exact immutable input admitted by this operation. Presentation sessions
    /// consume this snapshot instead of rediscovering dependencies on disk.
    pub fn source_files(&self) -> Vec<(String, String)> {
        self.files.clone()
    }

    /// Stable source-root owner identity carried into the session commit.
    pub fn source_set_id(&self) -> &str {
        &self.id
    }

    /// Address the exact prepared source files without using their volatile
    /// preparation revision or filesystem root path. Hashing is completed on
    /// the preparation worker before the compiler session commit.
    pub fn content_closure(
        &self,
    ) -> Result<ModelicaSourceRootContent, ModelicaSourceRootContentError> {
        self.content_closure.clone()
    }

    /// Prepare the installed paired artifact on the source-preparation worker.
    pub fn from_source_library(id: impl Into<String>) -> Self {
        let id = id.into();
        let label = lunco_assets_runtime::library::primary_filesystem_library_root()
            .map(|root| root.display().to_string())
            .unwrap_or_else(|| "installed source library".to_owned());
        match lunco_modelica_library::source_library::parsed_source_bundle() {
            Some(documents) => Self::from_parsed_documents(id, label, (**documents).clone()),
            None => Self::prepare(
                id,
                label,
                Vec::new(),
                vec!["installed paired source library is unavailable".to_owned()],
            ),
        }
    }

    /// Admit paired parsed records without reparsing their retained source bytes.
    fn from_parsed_documents(
        id: impl Into<String>,
        label: impl Into<String>,
        mut parsed: Vec<rumoca_compile::compile::ParsedSourceDocument>,
    ) -> Self {
        let id = id.into();
        let label = label.into();
        parsed.sort_unstable_by(|left, right| left.uri().cmp(right.uri()));
        let files = parsed
            .iter()
            .map(|document| (document.uri().to_owned(), document.source().to_owned()))
            .collect::<Vec<_>>();
        let diagnostics = if files.is_empty() {
            vec![format!("source root `{id}` contains no Modelica files")]
        } else {
            Vec::new()
        };
        let parsed_roots = source_roots_from_parsed_docs(&parsed);
        let content_closure = source_root_content_closure(&id, &label, &files, &diagnostics);
        Self {
            id,
            label,
            files,
            parsed,
            parsed_roots,
            diagnostics,
            content_closure,
        }
    }

    /// Prepare a complete source set without touching a compiler session.
    /// Files are canonicalized by URI so parser input and diagnostics
    /// do not depend on filesystem enumeration or
    /// worker completion order. Read errors are terminal for the source set;
    /// no parseable subset is published.
    pub fn prepare(
        id: impl Into<String>,
        label: impl Into<String>,
        mut files: Vec<(String, String)>,
        mut diagnostics: Vec<String>,
    ) -> Self {
        let id = id.into();
        let label = label.into();
        files.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if files.is_empty() && diagnostics.is_empty() {
            diagnostics.push(format!("source root `{id}` contains no Modelica files"));
        }

        let mut parsed = Vec::with_capacity(files.len());
        for (uri, text) in &files {
            match rumoca_compile::compile::ParsedSourceDocument::parse(uri.clone(), text.clone()) {
                Ok(ast) => {
                    parsed.push(ast);
                }
                Err(error) => diagnostics.push(format!(
                    "source root `{id}`: could not parse {uri}: {error:?}"
                )),
            }
        }

        let parsed_roots = source_roots_from_parsed_docs(&parsed);
        let content_closure = source_root_content_closure(&id, &label, &files, &diagnostics);
        Self {
            id,
            label,
            files,
            parsed,
            parsed_roots,
            diagnostics,
            content_closure,
        }
    }
}

/// Authored instance values classified by the compiler's admission owner.
#[derive(Clone, Debug, Default)]
pub struct AdmittedModelicaBindings {
    pub parameter_overrides: Vec<(String, f64)>,
    pub initial_inputs: Vec<(String, f64)>,
}

/// Compile Modelica models through one session-owned source-root admission
/// boundary and one strict reachable-DAE call. Source roots are admitted from
/// the parsed source before compilation; an unresolved dependency is a terminal
/// compile error rather than a signal to run the DAE pipeline again.
pub struct ModelicaCompiler {
    session: Session,
    last_compile_diagnostics: Option<(String, String, Vec<lunco_doc::Diagnostic>)>,
    /// Top-level package names whose source root is seated in this session —
    /// `"Modelica"`, any structured package under the application search path,
    /// and any other shipped library.
    ///
    /// This is the session's view of MODELICAPATH: Modelica looks up only the
    /// ROOT segment of a qualified name against the search path (`LunCo` is the
    /// root for `LunCo.Propulsion.BellNozzle`), loads that library once, and resolves
    /// everything below it inside the loaded tree. Keyed by that root segment for
    /// the same reason.
    ///
    /// It replaces separate per-library booleans, which recorded the same fact
    /// under hardcoded names and left every additional package without a place
    /// in the search path.
    ///
    /// Source-root installation records the top-level names it actually
    /// parsed here as well. The source-root command may be keyed by a mount
    /// identifier such as `twin:school`, while Modelica resolves a member by
    /// its authored `within` root such as `School`; recording the authored
    /// namespace is what lets later package-member compiles use the already
    /// seated root instead of registering a second URI.
    installed_roots: std::collections::HashSet<String>,
    /// Admitted source-set contributions retained so unloading one set can
    /// recompute roots, revisions, and content closures accurately.
    source_set_contributions: std::collections::HashMap<String, SourceSetContribution>,
    /// Source sets are replayed in their deterministic admission order.
    source_set_order: Vec<String>,
    /// Root segments referenced by the active source document. This is the
    /// compiler's parsed view of the current Modelica search path and keeps
    /// an unresolved reference from loading unrelated external packages.
    requested_source_roots: std::collections::HashSet<String>,
    /// URIs of the user documents currently seated as overlays in this
    /// reused session (NOT the resident source roots). Every compile is
    /// HERMETIC with respect to prior compiles: before seating its own
    /// document set, a compile evicts any previously-seated user doc that is
    /// not part of *this* compile. Without this, a prior compile's primary
    /// overlay stays resident, and compiling a SECOND document that defines
    /// the same package (e.g. the same model opened in two tabs / restored +
    /// shared) leaves the package registered under two URIs → rumoca's merge
    /// pass fails with `Duplicate class '…' found in '…' with non-identical
    /// definition`. The per-doc `session_uri` keying alone can't prevent this
    /// because the two docs legitimately have different URIs; the session
    /// itself must hold only the active compile's user docs.
    seated_user_uris: std::collections::HashSet<String>,
    /// Identity and selected class of the currently seated source unit. This
    /// lets parameter admission and strict compilation share the same parse.
    seated_source_unit: Option<(lunco_hash::content::Cid, String)>,
    /// Content revisions of the source roots admitted into this session.
    /// These are computed from bytes already read by the admission boundary;
    /// the prepared solve cache uses the aggregate without rescanning disk.
    library_revisions: std::collections::HashMap<String, u64>,
    /// Source-content state for committed roots and failed latest admissions.
    source_root_content_closures: std::collections::HashMap<
        String,
        Result<ModelicaSourceRootContent, ModelicaSourceRootContentError>,
    >,
    /// Roots whose explicit preparation failed. A live compile must report
    /// that result instead of retrying file reads synchronously on the worker.
    failed_source_roots: std::collections::HashMap<String, String>,
}

#[derive(Clone)]
struct SourceSetContribution {
    source_files: Vec<lunco_modelica_ast::resource_uri::SourceFileFacts>,
    roots: std::collections::HashSet<String>,
    revision: u64,
    content_closure: Result<ModelicaSourceRootContent, ModelicaSourceRootContentError>,
}

impl Default for ModelicaCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelicaCompiler {
    /// Construct an empty compiler session.
    ///
    /// Source roots are admitted explicitly through
    /// [`Self::ensure_source_bundle_installed`],
    /// [`Self::ensure_source_root_installed`], or one of the source-root load
    /// methods. This keeps source selection at the caller that owns the
    /// authored source-root declaration instead of making construction scan
    /// or select a particular library.
    pub fn new() -> Self {
        Self {
            session: Session::new(SessionConfig::default()),
            last_compile_diagnostics: None,
            installed_roots: std::collections::HashSet::new(),
            source_set_contributions: std::collections::HashMap::new(),
            source_set_order: Vec::new(),
            requested_source_roots: std::collections::HashSet::new(),
            seated_user_uris: std::collections::HashSet::new(),
            seated_source_unit: None,
            library_revisions: std::collections::HashMap::new(),
            source_root_content_closures: std::collections::HashMap::new(),
            failed_source_roots: std::collections::HashMap::new(),
        }
    }

    /// Install the resident parsed source bundle into this session. The bundle
    /// may contain any number of authored libraries; root identities are
    /// derived from their `within` declarations rather than selected by name.
    pub fn ensure_source_bundle_installed(&mut self) -> bool {
        if self.source_set_contributions.contains_key("source-bundle") {
            return true;
        }
        let report =
            self.install_source_root(PreparedSourceRoot::from_source_library("source-bundle"));
        report.diagnostics.is_empty() && report.inserted_file_count > 0
    }

    /// Seat a shipped Modelica library into this session by its TOP-LEVEL name,
    /// once. A root such as `LunCo` maps to the engine Modelica library's
    /// standard structured package (`package.mo` + `package.order` + members).
    ///
    /// This is MODELICAPATH lookup: the root segment of a qualified name names a
    /// library, the library is loaded whole, and everything below resolves inside
    /// the loaded tree from the members' own `within` declarations. Idempotent and
    /// cheap on repeat.
    ///
    /// The runtime asset tree is the source on native; browser consumers use the
    /// Bevy `ModelicaSource` loader. There is one source of truth, so an edited `.mo`
    /// cannot be compiled from a stale second copy. The disk tree is
    /// the one Bevy's AssetServer serves, so it is what `info:sourceAsset`
    /// already reads; taking the library from anywhere else would make an edited
    /// member compile as its last-built self while the scene loaded the new text.
    ///
    /// Original member declarations enter the same compiler source-set path as
    /// other libraries. Rumoca owns scoped runtime input initialization.
    pub fn ensure_source_root_installed(&mut self, root: &str) -> bool {
        if self.installed_roots.contains(root) {
            return true;
        }
        if self.failed_source_roots.contains_key(root) {
            return false;
        }
        let live_dir = match lunco_assets_core::models_package_root_path(root) {
            Ok(path) => path,
            Err(error) => {
                self.failed_source_roots
                    .insert(root.to_owned(), error.to_string());
                log::error!("[ModelicaCompiler] invalid model asset root: {error}");
                return false;
            }
        };
        if let Some(dir) = live_dir {
            let files = match lunco_assets_runtime::models::package_files(root) {
                Ok(files) if !files.is_empty() => files,
                result => {
                    let error = match result {
                        Ok(_) => "package has no Modelica source files".into(),
                        Err(error) => error,
                    };
                    log::error!("[ModelicaCompiler] source root `{root}` failed: {error}");
                    self.failed_source_roots.insert(root.to_owned(), error);
                    return false;
                }
            };
            let label = dir.display().to_string();
            log::info!(
                "[ModelicaCompiler] seated library `{root}` from the live asset tree ({})",
                files.len(),
            );
            let report = self.seat_library_files(root, &label, files);
            if !report.diagnostics.is_empty() || report.inserted_file_count == 0 {
                log::error!(
                    "[ModelicaCompiler] source root `{root}` failed: {}",
                    if report.diagnostics.is_empty() {
                        "no Modelica definitions were inserted".to_string()
                    } else {
                        report.diagnostics.join("; ")
                    },
                );
                return false;
            }
            self.installed_roots.insert(root.to_string());
            return true;
        }

        if self.ensure_source_bundle_installed() && self.installed_roots.contains(root) {
            return true;
        }

        // The same root-segment contract also covers a flat bundled model
        // (`Foo.mo` → root `Foo`). Use the existing bundled source path so
        // source-root admission covers both package and example entries in the
        // asset inventory.
        let filename = format!("{root}.mo");
        let Ok(Some(source)) = lunco_assets_runtime::models::model_source(&filename) else {
            return false;
        };
        log::info!("[ModelicaCompiler] seated bundled root `{root}` ({filename})",);
        let report = self.seat_library_files(
            root,
            &format!("bundled:{filename}"),
            vec![(filename, source)],
        );
        if !report.diagnostics.is_empty() || report.inserted_file_count == 0 {
            log::error!(
                "[ModelicaCompiler] bundled source root `{root}` failed: {}",
                if report.diagnostics.is_empty() {
                    "no Modelica definitions were inserted".to_string()
                } else {
                    report.diagnostics.join("; ")
                },
            );
            return false;
        }
        self.installed_roots.insert(root.to_string());
        true
    }

    /// Admit a library as one parsed source set.
    fn seat_library_files(
        &mut self,
        id: &str,
        label: &str,
        files: Vec<(String, String)>,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        self.load_source_root_in_memory(id, label, files)
    }

    /// Select translation-time overrides from Rumoca's checked declaration
    /// facts and convert the numeric transport to exact typed root modifiers.
    pub fn structural_parameter_overrides(
        dae: &rumoca_compile::compile::Dae,
        values: &[(String, f64)],
    ) -> Result<Vec<(String, rumoca_compile::compile::StructuralOverride)>, String> {
        use lunco_modelica_ast::resolved_interface::{ModelicaScalarType, ModelicaScalarValue};
        use rumoca_compile::compile::StructuralOverride;
        use rumoca_ir_dae::{ScalarType, VariableRole};
        let declarations = dae.inspect(|view| {
            view.variables()
                .filter(|(_, variable)| {
                    variable.role() == VariableRole::Parameter && !variable.is_tunable()
                })
                .flat_map(|(_, variable)| {
                    variable
                        .scalar_names()
                        .map(move |name| (name, variable.value_type().scalar_type()))
                })
                .collect::<std::collections::HashMap<_, _>>()
        });
        values
            .iter()
            .filter_map(|(name, value)| {
                declarations.get(name).map(|kind| {
                    let kind = match kind {
                        ScalarType::Real => ModelicaScalarType::Real,
                        ScalarType::Integer => ModelicaScalarType::Integer,
                        ScalarType::Boolean => ModelicaScalarType::Boolean,
                        _ => {
                            return Err(format!(
                                "structural parameter `{name}` has nonnumeric type {kind:?}"
                            ));
                        }
                    };
                    let literal = match ModelicaScalarValue::from_numeric(kind, *value)
                        .map_err(|error| format!("structural parameter `{name}`: {error}"))?
                    {
                        ModelicaScalarValue::Real(value) => StructuralOverride::Real(value),
                        ModelicaScalarValue::Integer(value) => StructuralOverride::Int(value),
                        ModelicaScalarValue::Boolean(value) => StructuralOverride::Bool(value),
                    };
                    Ok((name.clone(), literal))
                })
            })
            .collect()
    }

    /// Names whose scalar declaration bindings can be represented by Rumoca's
    /// root-modification API. Indexed scalar values belong to checked lowering.
    pub fn parameter_binding_names(values: &[(String, f64)]) -> Result<Vec<String>, String> {
        let mut names = std::collections::BTreeSet::new();
        for (name, _) in values {
            let segments = rumoca_core::qualified_name_segments(name).collect::<Vec<_>>();
            if segments.iter().any(|segment| segment.is_empty()) {
                return Err(format!(
                    "parameter binding `{name}` has an empty path segment"
                ));
            }
            if !segments
                .iter()
                .any(|segment| rumoca_core::parse_scalar_name(segment).is_some())
            {
                names.insert(name.clone());
            }
        }
        Ok(names.into_iter().collect())
    }

    fn parameter_root_bindings(
        &mut self,
        model_name: &str,
        values: &[(String, f64)],
    ) -> Result<Vec<(String, rumoca_compile::compile::StructuralOverride)>, String> {
        use lunco_modelica_ast::resolved_interface::ModelicaScalarValue;
        use rumoca_compile::compile::StructuralOverride;
        let names = Self::parameter_binding_names(values)?;
        let mut literals = Vec::with_capacity(names.len());
        for (name, value) in values {
            if names.binary_search(name).is_err() {
                continue;
            }
            let target = self
                .session
                .class_component_reference_query(model_name, name)
                .ok_or_else(|| {
                    format!(
                        "parameter binding `{name}` has no resolved declaration in `{model_name}`"
                    )
                })?;
            if !matches!(
                target.effective_variability,
                rumoca_core::Variability::Parameter(_)
            ) {
                return Err(format!(
                    "parameter binding `{name}` does not name a parameter"
                ));
            }
            let literal = match Self::raw_parameter_value(&target, *value)
                .map_err(|error| format!("parameter binding `{name}`: {error}"))?
            {
                ModelicaScalarValue::Real(value) => StructuralOverride::Real(value),
                ModelicaScalarValue::Integer(value) => StructuralOverride::Int(value),
                ModelicaScalarValue::Boolean(value) => StructuralOverride::Bool(value),
            };
            literals.push((name.clone(), literal));
        }
        Ok(literals)
    }

    /// Validate scalar overrides against the checked instance on cache hits and
    /// fresh compiles. Indexed values are solver overlays and cannot claim an
    /// unapplied structural modification.
    pub fn validate_parameter_bindings(
        dae: &rumoca_compile::compile::Dae,
        values: &[(String, f64)],
    ) -> Result<(), String> {
        use lunco_modelica_ast::resolved_interface::{ModelicaScalarType, ModelicaScalarValue};
        use rumoca_ir_dae::{ScalarType, VariableRole};
        let declarations = dae.inspect(|view| {
            view.variables()
                .flat_map(|(_, variable)| {
                    variable.scalar_names().map(move |name| {
                        (
                            name,
                            (
                                variable.role(),
                                variable.value_type().scalar_type(),
                                variable.is_tunable(),
                            ),
                        )
                    })
                })
                .collect::<std::collections::HashMap<_, _>>()
        });
        let root_names = Self::parameter_binding_names(values)?;
        for (name, value) in values {
            let Some((VariableRole::Parameter, kind, tunable)) = declarations.get(name) else {
                return Err(format!(
                    "parameter binding `{name}` is not a scalar parameter of the checked model"
                ));
            };
            if root_names.binary_search(name).is_err() && !*tunable {
                return Err(format!(
                    "indexed parameter `{name}` is structural; the root-modification API does not represent indexed structural bindings"
                ));
            }
            let kind = match kind {
                ScalarType::Real => ModelicaScalarType::Real,
                ScalarType::Integer => ModelicaScalarType::Integer,
                ScalarType::Boolean => ModelicaScalarType::Boolean,
                _ => {
                    return Err(format!(
                        "parameter binding `{name}` has nonnumeric type {kind:?}"
                    ));
                }
            };
            ModelicaScalarValue::from_numeric(kind, *value)
                .map_err(|error| format!("parameter binding `{name}`: {error}"))?;
        }
        Ok(())
    }

    /// Resolve only parameter candidates needed to construct the instance.
    /// Inputs, native sinks and absent conditional declarations are classified
    /// against the checked DAE after compilation, before numerical admission.
    pub fn admit_authored_bindings_after_source_root_admission(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
        values: &[(String, f64)],
        _external_inputs: &std::collections::BTreeSet<String>,
    ) -> Result<AdmittedModelicaBindings, String> {
        if values.is_empty() {
            return Ok(AdmittedModelicaBindings::default());
        }
        let selected = self.seat_source_unit(model_name, source, filename, extras, true)?;
        let mut admitted = AdmittedModelicaBindings::default();
        for (name, value) in values {
            if let Some(target) = self
                .session
                .class_component_reference_query(&selected, name)
                && matches!(
                    target.effective_variability,
                    rumoca_core::Variability::Parameter(_)
                )
            {
                Self::raw_parameter_value(&target, *value)
                    .map_err(|error| format!("authored parameter `{name}`: {error}"))?;
                admitted.parameter_overrides.push((name.clone(), *value));
            }
        }
        Ok(admitted)
    }

    /// Admit values only against the actual scalar instance. Separate native
    /// input ownership never turns a Modelica output into an input binding.
    pub fn admit_authored_bindings_from_checked_dae(
        dae: &rumoca_compile::compile::Dae,
        values: &[(String, f64)],
        external_inputs: &std::collections::BTreeSet<String>,
    ) -> Result<AdmittedModelicaBindings, String> {
        use lunco_modelica_ast::resolved_interface::ModelicaScalarValue;
        use rumoca_ir_dae::VariableRole;
        let declarations = dae.inspect(|view| {
            view.variables()
                .flat_map(|(_, variable)| {
                    variable.scalar_names().map(move |name| {
                        (name, (variable.role(), variable.value_type().scalar_type()))
                    })
                })
                .collect::<std::collections::HashMap<_, _>>()
        });
        let mut admitted = AdmittedModelicaBindings::default();
        for (name, value) in values {
            if !value.is_finite() {
                return Err(format!("authored binding `{name}` must be finite"));
            }
            match declarations.get(name) {
                Some((role @ (VariableRole::Parameter | VariableRole::Input), kind)) => {
                    let kind = Self::checked_scalar_type(*kind).ok_or_else(|| {
                        format!("authored binding `{name}` requires a numeric scalar")
                    })?;
                    ModelicaScalarValue::from_numeric(kind, *value)
                        .map_err(|error| format!("authored binding `{name}`: {error}"))?;
                    if *role == VariableRole::Parameter {
                        admitted.parameter_overrides.push((name.clone(), *value));
                    } else {
                        admitted.initial_inputs.push((name.clone(), *value));
                    }
                }
                _ if external_inputs.contains(name) => {}
                _ => {
                    return Err(format!(
                        "authored binding `{name}` is not a parameter or input of the checked model"
                    ));
                }
            }
        }
        Ok(admitted)
    }

    /// Compile source after admitting its source-root dependencies. Rumoca owns
    /// scope/reference resolution; `filename` identifies source diagnostics.
    pub fn compile_str(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.compile_str_mode(model_name, source, filename, false, &[])
    }

    /// Compile in the live worker's settled source-root context without file I/O.
    /// Rumoca owns scope and reference validation, including standalone `within`
    /// namespaces. Failed admitted roots retain their terminal diagnostics.
    pub fn compile_str_after_source_root_admission(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        parameter_bindings: &[(String, f64)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.compile_str_mode(model_name, source, filename, true, parameter_bindings)
    }

    fn compile_str_mode(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        source_roots_admitted: bool,
        parameter_bindings: &[(String, f64)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.compile_str_multi_mode(
            model_name,
            source,
            filename,
            &[],
            source_roots_admitted,
            parameter_bindings,
        )
    }

    /// Release user-document overlays after a caller has captured its compile
    /// result and diagnostics. Admitted library source roots remain installed.
    pub fn clear_user_documents(&mut self) {
        self.evict_user_docs_except(&std::collections::HashSet::new());
    }

    /// Remove every previously-seated user-document overlay whose URI is not
    /// in `keep`, so the reused session holds ONLY the active compile's user
    /// docs (plus the immutable source roots). This is what makes
    /// each compile hermetic against prior compiles — see
    /// [`Self::seated_user_uris`]. source library roots are installed via
    /// `replace_in_memory_source_set` (not tracked here), so they're untouched.
    fn evict_user_docs_except(&mut self, keep: &std::collections::HashSet<String>) {
        self.seated_source_unit = None;
        let stale: Vec<String> = self
            .seated_user_uris
            .iter()
            .filter(|uri| !keep.contains(*uri))
            .cloned()
            .collect();
        for uri in stale {
            self.session.remove_document(&uri);
            self.seated_user_uris.remove(&uri);
        }
    }

    /// Start a new owner request, including requests rejected before compilation.
    pub fn clear_compile_diagnostics(&mut self) {
        self.last_compile_diagnostics = None;
    }

    /// Consume located diagnostics from the original failed compile attempt.
    /// Source and requested class must match the exact request that produced it.
    pub fn take_compile_diagnostics(
        &mut self,
        model_name: &str,
        user_uri: &str,
    ) -> Vec<lunco_doc::Diagnostic> {
        match self.last_compile_diagnostics.take() {
            Some((model, uri, diagnostics)) if model == model_name && uri == user_uri => {
                diagnostics
            }
            _ => Vec::new(),
        }
    }

    /// Like `compile_str`, but seats additional `(filename, source)`
    /// pairs into the rumoca session before compiling so the resolver
    /// can satisfy cross-doc class references (e.g. a fresh untitled
    /// `RocketStage` referencing `AnnotatedRocketStage.Tank` from a
    /// sibling untitled doc that holds the package). Each extra is
    /// parsed once with its source text and installed in the same batch.
    /// Repeated URIs must carry identical source bytes.
    ///
    /// The active compile's user docs (primary + extras) become the session's
    /// ENTIRE user-overlay set: any previously-seated user doc that isn't part
    /// of this compile is evicted first (see [`Self::seated_user_uris`] /
    /// [`Self::evict_user_docs_except`]). This keeps the reused session
    /// hermetic — a prior compile of the same package under a different URI
    /// (e.g. the same model open in two tabs) can't linger and trip rumoca's
    /// `Duplicate class '…' found in '…' with non-identical definition`. The
    /// seated docs are left resident on return for further source queries;
    /// they're pruned by the NEXT
    /// compile, not removed here.
    pub fn compile_str_multi(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.compile_str_multi_mode(model_name, source, filename, extras, false, &[])
    }

    /// Compile primary and sibling documents in the live worker's settled roots.
    /// This path performs no file discovery; Rumoca validates all references.
    pub fn compile_str_multi_after_source_root_admission(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
        parameter_bindings: &[(String, f64)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.compile_str_multi_mode(
            model_name,
            source,
            filename,
            extras,
            true,
            parameter_bindings,
        )
    }

    /// Identity of the exact source unit and admitted compiler context.
    /// The worker owns retention of checked DAE artifacts for this context.
    pub fn source_unit_identity(
        &self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
    ) -> Result<lunco_hash::content::Cid, String> {
        let mut identity = b"LunCoModelicaSourceUnit\0\x01".to_vec();
        source_content_field(&mut identity, model_name.as_bytes());
        source_content_field(&mut identity, filename.as_bytes());
        source_content_field(
            &mut identity,
            &self.admitted_compilation_identity()?.to_bytes(),
        );
        let mut sources = std::collections::BTreeMap::from([(
            filename,
            lunco_hash::content::cid(source.as_bytes()),
        )]);
        for (uri, text) in extras {
            let content = lunco_hash::content::cid(text.as_bytes());
            if let Some(previous) = sources.insert(uri, content) {
                if previous != content {
                    return Err(format!(
                        "source URI `{uri}` has conflicting contents in one source unit"
                    ));
                }
            }
        }
        for (uri, content) in sources {
            source_content_field(&mut identity, uri.as_bytes());
            source_content_field(&mut identity, &content.to_bytes());
        }
        Ok(lunco_hash::content::cid(&identity))
    }

    fn compile_str_multi_mode(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
        source_roots_admitted: bool,
        parameter_bindings: &[(String, f64)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.clear_compile_diagnostics();
        let selected =
            self.seat_source_unit(model_name, source, filename, extras, source_roots_admitted)?;
        let root_bindings = self.parameter_root_bindings(&selected, parameter_bindings)?;
        if !root_bindings.is_empty() {
            self.session.set_structural_overrides(&root_bindings);
        }
        let result = self.compile_loaded(&selected);
        if !root_bindings.is_empty() {
            self.session.set_structural_overrides(&[]);
        }
        let compiled = match result {
            Ok(compiled) => compiled,
            Err(failure) => {
                self.last_compile_diagnostics = Some((
                    model_name.to_owned(),
                    filename.to_owned(),
                    diagnostics_from_strict_failure(&failure, filename),
                ));
                return Err(failure.summary);
            }
        };
        Self::validate_parameter_bindings(&compiled.dae, parameter_bindings)?;
        Ok(compiled)
    }

    /// Seat the exact source unit through the shared parsed-document/root owner.
    /// Parameter admission and DAE compilation use this same parsed input.
    fn seat_source_unit(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
        source_roots_admitted: bool,
    ) -> Result<String, String> {
        use rumoca_compile::compile::ParsedSourceDocument;
        let identity = if source_roots_admitted {
            Some(self.source_unit_identity(model_name, source, filename, extras)?)
        } else {
            None
        };
        if let (Some(identity), Some((seated, selected))) = (identity, &self.seated_source_unit)
            && identity == *seated
        {
            return Ok(selected.clone());
        }
        let primary = ParsedSourceDocument::parse(filename, source)
            .map_err(|error| format!("could not parse {filename}: {error}"))?;
        let selected = primary
            .definition()
            .qualified_class_name(model_name)
            .unwrap_or_else(|| model_name.to_owned());
        self.requested_source_roots =
            rumoca_compile::compile::source_root_dependencies(primary.definition())
                .into_iter()
                .collect();
        let mut documents = vec![(primary, lunco_hash::content::cid(source.as_bytes()))];
        let mut seen = std::collections::HashMap::from([(
            filename.to_owned(),
            lunco_hash::content::cid(source.as_bytes()),
        )]);
        for (uri, text) in extras {
            let content = lunco_hash::content::cid(text.as_bytes());
            if let Some(previous) = seen.get(uri) {
                if previous != &content {
                    return Err(format!(
                        "source URI `{uri}` has conflicting contents in one compile"
                    ));
                }
                continue;
            }
            let document = ParsedSourceDocument::parse(uri.clone(), text.clone())
                .map_err(|error| format!("could not parse {uri}: {error}"))?;
            self.requested_source_roots
                .extend(rumoca_compile::compile::source_root_dependencies(
                    document.definition(),
                ));
            seen.insert(uri.clone(), content);
            documents.push((document, content));
        }
        self.prepare_requested_source_roots(source_roots_admitted)?;
        let mut admitted = Vec::new();
        let mut keep = std::collections::HashSet::new();
        for (document, content) in documents {
            let declared = document.definition().declared_class_names();
            if declared.is_empty() {
                continue;
            }
            let owned = declared
                .iter()
                .filter(|name| self.class_is_owned_by_installed_root(name))
                .count();
            if owned > 0 {
                if owned != declared.len() {
                    return Err(format!(
                        "source `{}` mixes classes already owned by a loaded source root with new definitions; update the source root or use a distinct namespace",
                        document.uri()
                    ));
                }
                if !self.source_root_content_closures.values().any(|closure| {
                    closure
                        .as_ref()
                        .is_ok_and(|root| root.files.iter().any(|file| file.cid == content))
                }) {
                    return Err(format!(
                        "source `{}` declares classes already owned by a loaded source root with different contents; update the source-root document or use a distinct namespace",
                        document.uri()
                    ));
                }
                continue;
            }
            keep.insert(document.uri().to_owned());
            admitted.push(document);
        }
        self.evict_user_docs_except(&keep);
        self.session.add_in_memory_parsed_batch(admitted);
        self.seated_user_uris = keep;
        self.seated_source_unit = identity.map(|identity| (identity, selected.clone()));
        Ok(selected)
    }

    /// Whether a fully-qualified class name belongs to a durable source root
    /// already seated in this compiler session. User overlays are deliberately
    /// not treated as durable ownership; they remain governed by
    /// `seated_user_uris` and are replaced by the next compile.
    fn class_is_owned_by_installed_root(&mut self, qualified: &str) -> bool {
        let Some(root) = rumoca_core::qualified_name_segments(qualified).next() else {
            return false;
        };
        self.installed_roots.contains(root) && self.session.class_lookup_query(qualified).is_some()
    }

    /// Compile a source-library class that is already loaded into the session
    /// (no `update_document` call). Used by the
    /// `lunco-modelica-assets` indexer's `--warm` pass to populate rumoca's
    /// semantic-summary cache for common
    /// examples — the workbench's first compile of those classes is
    /// then a cache hit instead of paying the full multi-minute walk.
    pub fn compile_library_class(
        &mut self,
        qualified: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.clear_compile_diagnostics();
        self.requested_source_roots.clear();
        self.compile_loaded(qualified)
            .map_err(|failure| failure.summary)
    }

    /// Inner helper: heartbeat + session.compile + final timing log.
    /// Both [`Self::compile_str`] (user-edited source) and
    /// [`Self::compile_library_class`] (already-loaded source library class) flow
    /// through here so the heartbeat behaviour is identical.
    fn compile_loaded(
        &mut self,
        model_name: &str,
    ) -> Result<
        Box<rumoca_compile::compile::DaeCompilationResult>,
        Box<rumoca_compile::compile::StrictCompileFailure>,
    > {
        let t_total = web_time::Instant::now();

        // Native diagnostic progress is scoped to this exact compile. Its
        // guard interrupts and joins the heartbeat on return or unwind.
        #[cfg(not(target_arch = "wasm32"))]
        let _heartbeat = match compiler_heartbeat::CompileHeartbeat::start(model_name) {
            Ok(heartbeat) => Some(heartbeat),
            Err(error) => {
                log::warn!(
                    "[ModelicaCompiler] diagnostic heartbeat could not start for `{model_name}`: {error}"
                );
                None
            }
        };

        let result = self
            .session
            .compile_model_dae_strict_reachable_uncached_with_recovery_detailed(model_name);

        log::info!(
            "[ModelicaCompiler] compile `{}` finished in {:.2}s ({})",
            model_name,
            t_total.elapsed().as_secs_f64(),
            if result.is_ok() { "OK" } else { "ERR" },
        );
        result
    }

    /// Seat the source roots discoverable from the source text before the first
    /// compile attempt.  The compiler session is the sole owner of this
    /// operation; callers only provide source text and never duplicate the
    /// dependency inventory or root installation logic.
    fn prepare_requested_source_roots(
        &mut self,
        source_roots_admitted: bool,
    ) -> Result<(), String> {
        let mut roots = self
            .requested_source_roots
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        roots.sort_unstable();
        for root in roots {
            if let Some(error) = self.failed_source_roots.get(&root) {
                return Err(format!("Modelica source root `{root}` failed: {error}"));
            }
            if !source_roots_admitted && !self.installed_roots.contains(&root) {
                let _ = self.ensure_source_root_installed(&root);
            }
        }
        Ok(())
    }

    /// Record a source-root failure from the worker's explicit admission
    /// result. Later compiles referencing that root fail before a synchronous
    /// compiler-side retry can read or parse it again.
    pub fn record_source_root_failure(&mut self, id: impl Into<String>, error: String) {
        self.failed_source_roots.insert(id.into(), error);
    }

    /// Synchronous convenience for hosts that do not have a worker admission
    /// boundary. The live native Modelica worker prepares roots off-thread with
    /// [`PreparedSourceRoot::prepare`] and calls [`Self::install_source_root`]
    /// on its session owner. This method reads, prepares, and installs on the
    /// caller; it must not be used on a responsive application schedule.
    pub fn load_source_root(
        &mut self,
        id: &str,
        root_dir: &std::path::Path,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        // Asset discovery owns reads; admission retains exact contributing bytes.
        let (files, read_diagnostics) =
            lunco_assets_runtime::discovery::read_files_with_extension(root_dir, "mo");
        for diagnostic in &read_diagnostics {
            log::warn!("[ModelicaCompiler] source root `{id}`: {diagnostic}");
        }
        let diagnostics = read_diagnostics
            .into_iter()
            .map(|diagnostic| format!("source root `{id}`: {diagnostic}"))
            .collect();
        self.install_source_root(PreparedSourceRoot::prepare(
            id,
            root_dir.display().to_string(),
            files,
            diagnostics,
        ))
    }

    /// Synchronously prepare and install an in-memory source root. This is a
    /// convenience for small caller-owned inputs; native live workers should
    /// prepare through [`PreparedSourceRoot::prepare`] and commit through
    /// [`Self::install_source_root`]. Members are installed as one source set
    /// so a malformed member cannot leave a partially-installed root behind.
    ///
    /// `label` shows up in diagnostics as the "source root path"
    /// (rumoca convention: `"in-memory:<id>"`). `files` is a list
    /// of `(uri, source)` pairs; each `uri` is the filename rumoca
    /// will report errors against.
    ///
    /// Original declarations are retained for scope-aware DAE initialization.
    pub fn load_source_root_in_memory(
        &mut self,
        id: &str,
        label: &str,
        files: Vec<(String, String)>,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        self.install_source_root(PreparedSourceRoot::prepare(id, label, files, Vec::new()))
    }

    /// Commit a prepared source set into the session that owns Modelica
    /// compiler state. Failed file reads or parses leave both the session and
    /// contributing source contents unchanged.
    pub fn install_source_root(
        &mut self,
        prepared: PreparedSourceRoot,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        let PreparedSourceRoot {
            id,
            label,
            files,
            parsed,
            parsed_roots,
            mut diagnostics,
            content_closure,
        } = prepared;
        let source_files = parsed
            .iter()
            .map(|document| {
                lunco_modelica_ast::resource_uri::SourceFileFacts::parsed(
                    document.definition(),
                    document.uri(),
                )
            })
            .collect();
        let file_count = files.len();
        // A source root is one semantic unit. Do not publish a partial package:
        // the compile owner must either see every member or a terminal load
        // diagnostic. Bulk installation also keeps Rumoca's source-set index
        // and invalidation work to one pass instead of one pass per file.
        let inserted = if diagnostics.is_empty() && !parsed.is_empty() {
            let inserted = self.session.replace_in_memory_source_set(
                &id,
                rumoca_compile::compile::SourceRootKind::DurableExternal,
                parsed,
                None,
            );
            if inserted > 0 {
                if !self.source_set_contributions.contains_key(&id) {
                    self.source_set_order.push(id.clone());
                }
                let revision = source_set_revision(&id, &files);
                self.source_set_contributions.insert(
                    id.clone(),
                    SourceSetContribution {
                        source_files,
                        roots: parsed_roots,
                        revision,
                        content_closure: content_closure.clone(),
                    },
                );
                self.failed_source_roots.remove(&id);
                self.rebuild_source_set_indexes();
            }
            inserted
        } else {
            0
        };
        if diagnostics.is_empty() && inserted == 0 {
            diagnostics.push(format!(
                "source root `{id}` contained no Modelica definitions to install"
            ));
        }
        let report = rumoca_compile::compile::SourceRootLoadReport {
            source_set_id: id.clone(),
            source_root_path: label,
            parsed_file_count: file_count,
            inserted_file_count: inserted,
            cache_status: None,
            cache_key: None,
            cache_file: None,
            diagnostics,
        };
        if report.diagnostics.is_empty() && report.inserted_file_count > 0 {
            self.source_root_content_closures
                .insert(report.source_set_id.clone(), content_closure);
        } else {
            self.source_root_content_closures.insert(
                report.source_set_id.clone(),
                Err(ModelicaSourceRootContentError::SourceRootAdmissionFailed {
                    source_set_id: report.source_set_id.clone(),
                    details: report.diagnostics.join("; "),
                }),
            );
            self.failed_source_roots
                .insert(report.source_set_id.clone(), report.diagnostics.join("; "));
        }
        report
    }

    /// Authored namespaces belonging to one admitted source set. The source-set
    /// identity owns loading/lifetime; these names own qualified Modelica lookup.
    pub fn source_root_namespaces(&self, source_set_id: &str) -> Vec<String> {
        let mut namespaces = self
            .source_set_contributions
            .get(source_set_id)
            .map(|contribution| contribution.roots.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        namespaces.sort_unstable();
        namespaces
    }

    /// Package storage facts from the exact admitted ASTs. URI consumers must
    /// not infer the containing source from a package name or a cache layout.
    pub fn source_root_source_facts(
        &self,
        source_set_id: &str,
    ) -> Vec<lunco_modelica_ast::resource_uri::SourceFileFacts> {
        self.source_set_contributions
            .get(source_set_id)
            .map(|contribution| contribution.source_files.clone())
            .unwrap_or_default()
    }

    /// Remove one previously installed source set and recompute all derived
    /// source-root metadata without disturbing other loaded sets.
    pub fn remove_source_root(&mut self, source_set_id: &str) -> bool {
        self.session.remove_source_set(source_set_id);
        self.source_root_content_closures.remove(source_set_id);
        self.failed_source_roots.remove(source_set_id);
        let was_installed = self
            .source_set_contributions
            .remove(source_set_id)
            .is_some();
        self.source_set_order
            .retain(|candidate| candidate != source_set_id);
        self.rebuild_source_set_indexes();
        was_installed
    }

    fn rebuild_source_set_indexes(&mut self) {
        self.installed_roots.clear();
        self.library_revisions.clear();
        self.source_root_content_closures.clear();

        for source_set_id in &self.source_set_order {
            let Some(contribution) = self.source_set_contributions.get(source_set_id) else {
                continue;
            };
            self.installed_roots
                .extend(contribution.roots.iter().cloned());
            self.library_revisions
                .insert(source_set_id.clone(), contribution.revision);
            self.source_root_content_closures
                .insert(source_set_id.clone(), contribution.content_closure.clone());
        }
    }

    /// Return one deterministic revision for the complete set of admitted
    /// source roots. The ordering of the source-root map is not semantic, so
    /// root identifiers are sorted before hashing.
    pub fn library_revision(&self) -> u64 {
        use std::hash::{Hash, Hasher};

        let mut roots = self
            .library_revisions
            .iter()
            .map(|(id, revision)| (id.as_str(), *revision))
            .collect::<Vec<_>>();
        roots.sort_unstable_by(|left, right| left.0.cmp(right.0));

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        SOURCE_SET_REVISION_VERSION.hash(&mut hasher);
        for (id, revision) in roots {
            id.hash(&mut hasher);
            revision.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Exact admitted context for serialized compiler artifacts, including
    /// source-set identities and diagnostic URIs. Source admission order is
    /// semantic; an artifact cannot carry provenance from another admission.
    pub fn admitted_compilation_identity(&self) -> Result<lunco_hash::content::Cid, String> {
        if !self.failed_source_roots.is_empty() {
            return Err("the latest source-root admission failed".into());
        }
        let mut bytes = b"LunCoModelicaAdmittedCompilationContext\0\x01".to_vec();
        for id in &self.source_set_order {
            let contribution = self
                .source_set_contributions
                .get(id)
                .ok_or("admitted source-set order has no contribution")?;
            let content = contribution
                .content_closure
                .as_ref()
                .map_err(|error| error.to_string())?;
            source_content_field(&mut bytes, id.as_bytes());
            source_content_field(&mut bytes, &(content.files.len() as u64).to_le_bytes());
            for file in &content.files {
                source_content_field(&mut bytes, file.source_uri.as_bytes());
                source_content_field(&mut bytes, &file.cid.to_bytes());
            }
            source_content_field(
                &mut bytes,
                &(contribution.source_files.len() as u64).to_le_bytes(),
            );
            for file in &contribution.source_files {
                source_content_field(&mut bytes, file.source_uri.as_bytes());
            }
        }
        Ok(lunco_hash::content::cid(&bytes))
    }

    /// Return the current source-content state for one root. Failed latest
    /// admissions return explicit errors rather than exposing stale source text
    /// as a closure.
    pub fn source_root_content_closure(
        &self,
        source_set_id: &str,
    ) -> Option<Result<&ModelicaSourceRootContent, &ModelicaSourceRootContentError>> {
        self.source_root_content_closures
            .get(source_set_id)
            .map(Result::as_ref)
    }

    /// Portable identity of the successfully seated compile input closure.
    /// Includes the exact target, seated source bytes, and admitted library
    /// bytes. Runtime mount IDs and host paths never contribute to this CID.
    /// Capture before clearing the admitted user-document overlay.
    pub fn compiled_source_content_identity(
        &self,
        compiled: &rumoca_compile::compile::DaeCompilationResult,
    ) -> Result<lunco_hash::content::Cid, String> {
        let closure = compiled
            .source_closure
            .as_ref()
            .ok_or("compile result has no strict participating-source closure")?;
        if closure.files.is_empty() {
            return Err("strict compile has no participating source files".into());
        }
        let mut bytes = b"LunCoModelicaCompileClosure\0\x01".to_vec();
        source_content_field(&mut bytes, closure.target.as_bytes());
        let mut overlays = Vec::new();
        let mut contributing_roots = std::collections::HashSet::new();
        for participant in &closure.files {
            if self.seated_user_uris.contains(&participant.uri) {
                let document = self
                    .session
                    .document_source_text(&participant.uri)
                    .ok_or("participating overlay no longer has seated text")?;
                overlays.push(lunco_hash::content::cid(document.as_bytes()));
            } else {
                if participant.source_set_keys.is_empty() {
                    return Err(format!(
                        "participating source `{}` has no admitted byte owner",
                        participant.uri
                    ));
                }
                contributing_roots.extend(participant.source_set_keys.iter().cloned());
            }
        }
        overlays.sort_by_key(|cid| cid.to_bytes());
        for cid in overlays {
            source_content_field(&mut bytes, &cid.to_bytes());
        }
        // Preserve actual first-wins admission order while excluding unrelated
        // roots. A contributing source set is conservatively represented in full.
        let mut ordered = self
            .source_set_order
            .iter()
            .filter(|id| contributing_roots.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        let mut remaining = contributing_roots
            .into_iter()
            .filter(|id| !ordered.contains(id))
            .collect::<Vec<_>>();
        remaining.sort();
        ordered.extend(remaining);
        for id in ordered {
            let content = self
                .source_root_content_closures
                .get(&id)
                .ok_or_else(|| format!("participating source root `{id}` has no byte identity"))?
                .as_ref()
                .map_err(|error| error.to_string())?;
            for file in &content.files {
                source_content_field(&mut bytes, file.source_uri.as_bytes());
                source_content_field(&mut bytes, &file.cid.to_bytes());
            }
        }
        Ok(lunco_hash::content::cid(&bytes))
    }
}

/// Convert a rumoca [`StrictCompileFailure`] into the Diagnostics
/// panel's [`Diagnostic`](lunco_doc::Diagnostic) form.
///
/// A failure becomes *located* (click-to-source) only when its primary
/// label points at `user_uri` — diagnostics rooted in source library / sibling
/// library files keep their message (suffixed with the originating
/// file name) but no line/column, so clicking never jumps the editor
/// to a file it isn't showing. Each message is prefixed with rumoca's
/// diagnostic code when present (`ED008`, `EI001`, …) so the panel
/// reads like a real compiler's output.
///
/// [`StrictCompileFailure`]: rumoca_compile::compile::StrictCompileFailure
fn diagnostics_from_strict_failure(
    report: &rumoca_compile::compile::StrictCompileFailure,
    user_uri: &str,
) -> Vec<lunco_doc::Diagnostic> {
    use lunco_doc::Diagnostic;
    report
        .failures
        .iter()
        .map(|f| {
            let message = match &f.error_code {
                Some(code) if !code.is_empty() => format!("[{code}] {}", f.error),
                _ => f.error.clone(),
            };
            // Resolve the primary span against its original compilation source map.
            let resolved = f.primary_label.as_ref().and_then(|label| {
                let (name, content) = report.source_map.get_source(label.span.source)?;
                Some((name.to_string(), content.to_string(), label.span.start.0))
            });
            match resolved {
                Some((name, content, start)) if name == user_uri => {
                    let (line, column) =
                        lunco_modelica_document::document::core::byte_offset_to_line_col(
                            &content, start,
                        );
                    Diagnostic::error(message, Some(line), Some(column))
                }
                // Diagnostic in another file — name it so the user knows
                // where it came from, but leave it unlocated.
                Some((name, _, _)) => Diagnostic::message_only(format!("{message}  (in {name})")),
                None => Diagnostic::message_only(message),
            }
        })
        .collect()
}

#[cfg(test)]
mod source_root_smoke {
    use super::*;

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn source_root_content_closure_keeps_package_read_failures_terminal() {
        const CHILD: &str = "LUNCO_PACKAGE_FAILURE_TEST";
        if std::env::var_os(CHILD).is_some() {
            let mut compiler = ModelicaCompiler::new();
            assert!(!compiler.ensure_source_root_installed("Probe"));
            let failure = compiler
                .failed_source_roots
                .get("Probe")
                .expect("present invalid package must retain its owning read failure");
            assert!(failure.contains("Bad.mo"), "{failure}");
            return;
        }
        let folder = tempfile::tempdir().unwrap();
        let package = folder.path().join("models").join("Probe");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.mo"), "package Probe end Probe;").unwrap();
        std::fs::write(package.join("Bad.mo"), [0xff]).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_root_smoke::source_root_content_closure_keeps_package_read_failures_terminal", "--nocapture"])
            .env(CHILD, "1")
            .env(lunco_assets_core::ASSET_ROOT_ENV, folder.path())
            .output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn source_root_content_closure_preserves_native_discovery_paths() {
        let folder = tempfile::tempdir().unwrap();
        let mut root = folder.path().join("source # % 日本語");
        // Exercise native long-path I/O as well as verbatim prefix parsing.
        for _ in 0..4 {
            root = root.join("directory".repeat(8));
        }
        let child = root.join("nested");
        std::fs::create_dir_all(&child).unwrap();
        let source = "within Probe; model Demo Real x; equation x = 1; end Demo;";
        std::fs::write(child.join("Demo.mo"), source).unwrap();
        // Canonicalization returns a verbatim path on Windows. Discovery must
        // preserve that native spelling until the compiler removes the root.
        let canonical = lunco_storage::canonicalize_file_path(&root).unwrap();
        let mut compiler = ModelicaCompiler::new();
        let mut expected = None;
        for root in [&root, &canonical] {
            let (files, diagnostics) =
                lunco_assets_runtime::discovery::read_files_with_extension(root, "mo");
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            assert_eq!(files.len(), 1);
            assert_eq!(
                std::path::Path::new(&files[0].0),
                root.join("nested/Demo.mo")
            );
            let prepared = PreparedSourceRoot::prepare(
                "Probe",
                root.display().to_string(),
                files,
                diagnostics,
            );
            let closure = prepared
                .content_closure()
                .expect("portable discovered content");
            assert_eq!(closure.files[0].source_uri, "nested/Demo.mo");
            assert_eq!(
                closure.files[0].cid,
                lunco_hash::content::cid(source.as_bytes())
            );
            if let Some(expected) = &expected {
                assert_eq!(&closure, expected);
            }
            expected = Some(closure);
            let report = compiler.install_source_root(prepared);
            assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
            compiler
                .compile_str(
                    "Consumer",
                    "model Consumer Probe.Demo demo; end Consumer;",
                    "Consumer.mo",
                )
                .unwrap();
        }
        #[cfg(windows)]
        for root in [
            r"C:\models",
            r"\\?\C:\models",
            r"\\server\share\models",
            r"\\?\UNC\server\share\models",
        ] {
            let path = std::path::Path::new(root).join("nested/Demo.mo");
            let closure = source_root_content_closure(
                "Probe",
                root,
                &[(path.to_str().unwrap().to_owned(), source.to_owned())],
                &[],
            )
            .unwrap();
            assert_eq!(Some(closure), expected);
        }
    }

    #[test]
    fn source_root_content_closure_is_stable_exact_and_portable() {
        let first_root = std::env::temp_dir().join("luncosim-modelica-root-one");
        let second_root = std::env::temp_dir().join("luncosim-modelica-root-two");
        let first = PreparedSourceRoot::prepare(
            "DiskDemo",
            first_root.display().to_string(),
            vec![
                (
                    first_root.join("z/Zed.mo").display().to_string(),
                    "model Zed end Zed;".into(),
                ),
                (
                    first_root.join("a/Alpha.mo").display().to_string(),
                    "model Alpha end Alpha;".into(),
                ),
            ],
            Vec::new(),
        );
        let reordered = PreparedSourceRoot::prepare(
            "DiskDemo",
            second_root.display().to_string(),
            vec![
                (
                    second_root.join("a/Alpha.mo").display().to_string(),
                    "model Alpha end Alpha;".into(),
                ),
                (
                    second_root.join("z/Zed.mo").display().to_string(),
                    "model Zed end Zed;".into(),
                ),
            ],
            Vec::new(),
        );

        let closure = first.content_closure().expect("complete source root");
        assert_eq!(closure, reordered.content_closure().expect("same files"));
        let backslash_paths = PreparedSourceRoot::prepare(
            "DiskDemo",
            "test-root",
            vec![
                (r"z\Zed.mo".into(), "model Zed end Zed;".into()),
                (r"a\Alpha.mo".into(), "model Alpha end Alpha;".into()),
            ],
            Vec::new(),
        );
        assert_eq!(
            closure,
            backslash_paths.content_closure().expect("portable paths")
        );
        assert_eq!(
            closure
                .files
                .iter()
                .map(|file| file.source_uri.as_str())
                .collect::<Vec<_>>(),
            ["a/Alpha.mo", "z/Zed.mo"]
        );
        assert_eq!(
            closure.files[0].cid,
            lunco_hash::content::cid(b"model Alpha end Alpha;")
        );

        let changed = PreparedSourceRoot::prepare(
            "DiskDemo",
            first_root.display().to_string(),
            vec![(
                first_root.join("a/Alpha.mo").display().to_string(),
                "model Alpha end Alpha; ".into(),
            )],
            Vec::new(),
        )
        .content_closure()
        .expect("changed source remains addressable");
        assert_ne!(closure.files[0].cid, changed.files[0].cid);
    }

    #[test]
    fn source_root_content_closure_rejects_incomplete_or_nonportable_sources() {
        let valid = "model Demo end Demo;".to_owned();
        let diagnosed = PreparedSourceRoot::prepare(
            "Demo",
            "test-root",
            vec![("Demo.mo".into(), valid.clone())],
            vec!["source read failed".into()],
        );
        assert_eq!(
            diagnosed.content_closure(),
            Err(ModelicaSourceRootContentError::SourceSetHasDiagnostics)
        );

        let empty_identity = PreparedSourceRoot::prepare(
            "",
            "test-root",
            vec![("Demo.mo".into(), valid.clone())],
            Vec::new(),
        );
        assert_eq!(
            empty_identity.content_closure(),
            Err(ModelicaSourceRootContentError::EmptySourceSetId)
        );

        let duplicate = PreparedSourceRoot::prepare(
            "Demo",
            "test-root",
            vec![
                ("Demo.mo".into(), "model A end A;".into()),
                ("Demo.mo".into(), "model B end B;".into()),
            ],
            Vec::new(),
        );
        assert_eq!(
            duplicate.content_closure(),
            Err(ModelicaSourceRootContentError::DuplicateFileUri {
                source_uri: "Demo.mo".into(),
            })
        );

        let outside_root = PreparedSourceRoot::prepare(
            "Demo",
            std::env::temp_dir()
                .join("luncosim-modelica-root-one")
                .display()
                .to_string(),
            vec![(
                std::env::temp_dir()
                    .join("luncosim-modelica-root-two/Demo.mo")
                    .display()
                    .to_string(),
                valid,
            )],
            Vec::new(),
        );
        assert!(matches!(
            outside_root.content_closure(),
            Err(ModelicaSourceRootContentError::NonPortableFileUri { .. })
        ));

        let machine_local_identity = PreparedSourceRoot::prepare(
            std::env::temp_dir().display().to_string(),
            "test-root",
            vec![("Demo.mo".into(), "model Demo end Demo;".into())],
            Vec::new(),
        );
        assert!(matches!(
            machine_local_identity.content_closure(),
            Err(ModelicaSourceRootContentError::NonPortableSourceSetId { .. })
        ));

        let windows_identity = PreparedSourceRoot::prepare(
            r"C:\Modelica",
            "test-root",
            vec![("Demo.mo".into(), "model Demo end Demo;".into())],
            Vec::new(),
        );
        assert!(matches!(
            windows_identity.content_closure(),
            Err(ModelicaSourceRootContentError::NonPortableSourceSetId { .. })
        ));

        let traversal_identity = PreparedSourceRoot::prepare(
            "../Modelica",
            "test-root",
            vec![("Demo.mo".into(), "model Demo end Demo;".into())],
            Vec::new(),
        );
        assert!(matches!(
            traversal_identity.content_closure(),
            Err(ModelicaSourceRootContentError::NonPortableSourceSetId { .. })
        ));

        for uri in ["file:///work/Demo.mo", "FILE:/work/Demo.mo", r"C:Demo.mo"] {
            let nonportable_uri = PreparedSourceRoot::prepare(
                "Demo",
                "test-root",
                vec![(uri.into(), "model Demo end Demo;".into())],
                Vec::new(),
            );
            assert!(matches!(
                nonportable_uri.content_closure(),
                Err(ModelicaSourceRootContentError::NonPortableFileUri { .. })
            ));
        }
    }

    #[test]
    fn compiler_source_root_content_closure_tracks_latest_admission_state() {
        let mut compiler = ModelicaCompiler::new();
        let prepared = PreparedSourceRoot::prepare(
            "Demo",
            "test-root",
            vec![("Demo.mo".into(), "model Demo end Demo;".into())],
            Vec::new(),
        );
        let expected = prepared.content_closure().expect("prepared content");
        assert!(compiler.source_root_content_closure("Demo").is_none());

        let report = compiler.install_source_root(prepared);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_eq!(
            compiler
                .source_root_content_closure("Demo")
                .expect("committed source root")
                .expect("exact source text retained"),
            &expected
        );

        let failed_replacement = PreparedSourceRoot::prepare(
            "Demo",
            "test-root",
            vec![("Demo.mo".into(), "model Broken".into())],
            vec!["source read failed".into()],
        );
        assert!(
            !compiler
                .install_source_root(failed_replacement)
                .diagnostics
                .is_empty()
        );
        let latest_state = compiler
            .source_root_content_closure("Demo")
            .expect("latest admission state");
        assert!(matches!(
            latest_state,
            Err(&ModelicaSourceRootContentError::SourceRootAdmissionFailed { .. })
        ));
    }

    #[test]
    fn runtime_input_defaults_follow_declaration_scope() {
        let mut compiler = ModelicaCompiler::new();
        let library = PreparedSourceRoot::prepare(
            "Controls",
            "inline library",
            vec![(
                "Controls/package.mo".into(),
                "package Controls
model On input Real enabled=1.0; output Real y; equation y=enabled; end On;
model Off input Real enabled=0.0; output Real y; equation y=enabled; end Off;
model Base input Real enabled=5.0; output Real y; equation y=enabled; end Base;
model Derived extends Base; end Derived;
end Controls;"
                    .into(),
            )],
            Vec::new(),
        );
        assert!(compiler.install_source_root(library).diagnostics.is_empty());
        let compiled = compiler.compile_str("Root",
            "model Root input Real enabled=9.0; input Controls.On a; input Controls.On again; input Controls.Off b; input Controls.Derived d; output Real y; equation y=enabled+a.y+again.y+b.y+d.y; end Root;", "root.mo")
            .expect("scoped defaults compile");
        for (name, value) in [
            ("enabled", 9.0),
            ("a.enabled", 1.0),
            ("again.enabled", 1.0),
            ("b.enabled", 0.0),
            ("d.enabled", 5.0),
        ] {
            compiled.dae.inspect(|view| {
                let (_, variable) = view
                    .variables()
                    .find(|(_, variable)| variable.name().as_str() == name)
                    .expect("qualified input retained");
                assert_eq!(
                    variable.role(),
                    rumoca_compile::compile::VariableRole::Input
                );
                let expression = variable
                    .binding()
                    .or_else(|| variable.start())
                    .expect("authored default retained");
                let actual = view.expression(expression).expect("checked expression");
                let rumoca_compile::compile::ExpressionOperation::Literal(
                    rumoca_ir_dae::DaeLiteral::Real(actual),
                ) = actual.operation()
                else {
                    panic!("{name}: authored literal default lost");
                };
                assert_eq!(*actual, value, "{name} retains its declaring class default");
            });
        }
        let internal = compiler.compile_str("Internal",
            "model Internal Controls.On a; Controls.Off b; output Real y; equation y=a.y+b.y; end Internal;", "internal.mo")
            .expect("internal defaults retain their equations");
        assert!(internal.dae.inspect(|view| {
            view.variables().all(|(_, variable)| {
                variable.role() != rumoca_compile::compile::VariableRole::Input
            })
        }));
        assert!(compiler.compile_str("Invalid",
            "model Invalid input Real enabled=missing; output Real y; equation y=enabled; end Invalid;", "invalid.mo").is_err(),
            "invalid defaults produce a diagnostic rather than a fabricated value");
    }

    #[test]
    fn unloading_one_source_set_rebuilds_only_its_compiler_contribution() {
        let mut compiler = ModelicaCompiler::new();
        let application = PreparedSourceRoot::prepare(
            "application-controls",
            "inline application source",
            vec![(
                "ApplicationControls/Rate.mo".into(),
                "within ApplicationControls;\nmodel Rate\n  input Real retained = 2.0;\n  output Real y;\nequation\n  y = retained;\nend Rate;\n".into(),
            )],
            Vec::new(),
        );
        let twin = PreparedSourceRoot::prepare(
            "twin:17:lesson:modelica:0",
            "inline Twin source",
            vec![(
                "LessonModelica/Rate.mo".into(),
                "within LessonModelica;\nmodel Rate\n  input Real retired = 7.0;\n  output Real y;\nequation\n  y = retired;\nend Rate;\n".into(),
            )],
            Vec::new(),
        );

        assert!(
            compiler
                .install_source_root(application)
                .diagnostics
                .is_empty()
        );
        assert!(compiler.install_source_root(twin).diagnostics.is_empty());
        assert!(compiler.installed_roots.contains("ApplicationControls"));
        assert!(compiler.installed_roots.contains("LessonModelica"));
        assert_eq!(
            compiler.source_root_namespaces("twin:17:lesson:modelica:0"),
            vec!["LessonModelica"]
        );
        let revision_before = compiler.library_revision();

        assert!(compiler.remove_source_root("twin:17:lesson:modelica:0"));

        assert!(!compiler.installed_roots.contains("LessonModelica"));
        assert!(
            compiler
                .source_root_namespaces("twin:17:lesson:modelica:0")
                .is_empty()
        );
        assert!(compiler.installed_roots.contains("ApplicationControls"));
        assert!(
            compiler
                .source_root_content_closure("twin:17:lesson:modelica:0")
                .is_none()
        );
        assert_ne!(compiler.library_revision(), revision_before);
    }

    #[test]
    fn source_root_reports_malformed_input_declarations() {
        let mut compiler = ModelicaCompiler::new();
        let report = compiler.load_source_root_in_memory(
            "Broken",
            "test-root",
            vec![(
                "Broken.mo".into(),
                "model Broken\n  input Real x = 1.0;\n".into(),
            )],
        );
        assert_eq!(report.parsed_file_count, 1);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|message| message.contains("could not parse Broken.mo")),
            "a malformed source root must not report Ready: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn source_root_failure_does_not_publish_partial_members() {
        let mut compiler = ModelicaCompiler::new();
        let report = compiler.load_source_root_in_memory(
            "Demo",
            "test-root",
            vec![
                (
                    "Demo/Healthy.mo".into(),
                    "model Healthy end Healthy;".into(),
                ),
                ("Demo/Broken.mo".into(), "model Broken".into()),
            ],
        );
        assert_eq!(report.parsed_file_count, 2);
        assert_eq!(report.inserted_file_count, 0);
        assert!(!report.diagnostics.is_empty());
        let result = compiler.compile_str(
            "Consumer",
            "model Consumer\n  Demo.Healthy healthy;\nend Consumer;",
            "Consumer.mo",
        );
        assert!(
            result.is_err(),
            "a failed source-root admission must not expose only its parseable members"
        );
    }

    #[test]
    fn prepared_source_root_canonicalizes_files_before_parsing() {
        let prepared = PreparedSourceRoot::prepare(
            "Demo",
            "test-root",
            vec![
                ("z/Zed.mo".into(), "model Zed end Zed;".into()),
                ("a/Alpha.mo".into(), "model Alpha end Alpha;".into()),
            ],
            Vec::new(),
        );

        assert_eq!(
            prepared
                .files
                .iter()
                .map(|(uri, _)| uri.as_str())
                .collect::<Vec<_>>(),
            ["a/Alpha.mo", "z/Zed.mo"]
        );
        assert_eq!(
            prepared
                .parsed
                .iter()
                .map(|document| document.uri())
                .collect::<Vec<_>>(),
            ["a/Alpha.mo", "z/Zed.mo"]
        );
        assert!(
            prepared.diagnostics.is_empty(),
            "{:?}",
            prepared.diagnostics
        );
    }

    #[test]
    fn source_root_read_error_prevents_installing_parseable_members() {
        let mut compiler = ModelicaCompiler::new();
        let prepared = PreparedSourceRoot::prepare(
            "Demo",
            "test-root",
            vec![(
                "Demo/Healthy.mo".into(),
                "model Healthy end Healthy;".into(),
            )],
            vec!["source root `Demo`: unreadable Demo/Broken.mo".into()],
        );
        let report = compiler.install_source_root(prepared);

        assert_eq!(report.parsed_file_count, 1);
        assert_eq!(report.inserted_file_count, 0);
        assert_eq!(report.diagnostics.len(), 1);
        let result = compiler.compile_str(
            "Consumer",
            "model Consumer\n  Demo.Healthy healthy;\nend Consumer;",
            "Consumer.mo",
        );
        assert!(
            result.is_err(),
            "a read error must not expose a partial source root"
        );
    }

    #[test]
    fn releasing_user_documents_keeps_admitted_library_source_roots() {
        let mut compiler = ModelicaCompiler::new();
        let prepared = PreparedSourceRoot::prepare(
            "application-library",
            "inline-library",
            vec![(
                "Shared.mo".into(),
                "package Shared model Component Real x = 1; end Component; end Shared;".into(),
            )],
            Vec::new(),
        );
        assert!(
            compiler
                .install_source_root(prepared)
                .diagnostics
                .is_empty()
        );
        let result = compiler.compile_str_multi(
            "Ephemeral",
            "model Ephemeral Real x = 1; end Ephemeral;",
            "Ephemeral.mo",
            &[],
        );
        assert!(result.is_ok());
        compiler.clear_user_documents();
        assert!(compiler.seated_user_uris.is_empty());
        assert!(compiler.session.class_lookup_query("Ephemeral").is_none());
        assert!(
            compiler
                .session
                .class_lookup_query("Shared.Component")
                .is_some()
        );
    }

    #[test]
    fn flat_source_root_reuses_exact_content_and_rejects_conflicting_definition() {
        let mut compiler = ModelicaCompiler::new();
        let source = "model Probe output Real x; equation x = 2; end Probe;";
        let report = compiler.load_source_root_in_memory(
            "twin:probe",
            "inline",
            vec![("Probe # %.mo".into(), source.into())],
        );
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert!(
            compiler
                .compile_str("Probe", source, "twin://probe/Probe # %.mo")
                .is_ok()
        );
        let error = compiler
            .compile_str(
                "Probe",
                "model Probe output Real x; equation x = 3; end Probe;",
                "other.mo",
            )
            .unwrap_err();
        assert!(error.contains("already owned"), "{error}");
    }

    #[test]
    fn source_root_namespace_owns_later_package_member_compiles() {
        let mut compiler = ModelicaCompiler::new();
        let member = "within Demo;\nmodel Part\n  Real x;\nequation\n  x = 1;\nend Part;\n";
        let report = compiler.load_source_root_in_memory(
            "twin:demo",
            "in-memory:demo",
            vec![
                (
                    "demo/package.mo".to_string(),
                    "within; package Demo end Demo;".to_string(),
                ),
                ("demo/Part.mo".to_string(), member.to_string()),
            ],
        );
        assert_eq!(report.inserted_file_count, 2);
        assert_eq!(compiler.source_root_namespaces("twin:demo"), vec!["Demo"]);
        assert!(report.diagnostics.is_empty(), "{report:?}");

        // The source root is keyed by a transport id (`twin:demo`), but the
        // package-member route must resolve the authored `within Demo;` root
        // and compile the already-seated class rather than adding a second
        // URI for the same qualified name.
        let result = compiler.compile_str("Part", member, "workspace/Part.mo");
        assert!(
            result.is_ok(),
            "a loaded source root must own its package members: {:?}",
            result.err()
        );

        let multi_result = compiler.compile_str_multi("Part", member, "workspace/Part.mo", &[]);
        assert!(
            multi_result.is_ok(),
            "the multi-document path must share source-root ownership: {:?}",
            multi_result.err()
        );
    }

    fn admission_fixture(root: &str, member: &str) -> ModelicaCompiler {
        let mut compiler = ModelicaCompiler::new();
        let report = compiler.load_source_root_in_memory(
            "inline:admission",
            "inline source admission",
            vec![
                ("package.mo".into(), format!("package {root} end {root};")),
                ("member.mo".into(), member.into()),
            ],
        );
        assert!(report.diagnostics.is_empty(), "{report:?}");
        compiler
    }

    #[test]
    fn source_admission_rejects_changed_owned_member() {
        let original = "within Demo; model Part output Real x=1; end Part;";
        let changed = "within Demo; model Part output Real x=2; end Part;";
        let mut compiler = admission_fixture("Demo", original);
        let single = compiler.compile_str("Demo.Part", changed, "edited.mo");
        let multi = compiler.compile_str_multi("Demo.Part", changed, "edited.mo", &[]);
        assert!(
            single.is_err() && multi.is_err(),
            "changed library source must be rejected: single={}, multi={}",
            single.is_err(),
            multi.is_err()
        );
        assert!(single.unwrap_err().contains("already owned"));
        assert!(multi.unwrap_err().contains("already owned"));
    }

    #[test]
    fn source_admission_accepts_new_within_namespace() {
        let source = "within FreeScope; model Part output Real x=1; end Part;";
        let mut compiler = ModelicaCompiler::new();
        let single = compiler.compile_str("FreeScope.Part", source, "new.mo");
        let multi = compiler.compile_str_multi("FreeScope.Part", source, "new.mo", &[]);
        assert!(
            single.is_ok(),
            "new namespace must compile: {:?}",
            single.err()
        );
        assert!(
            multi.is_ok(),
            "new namespace must compile: {:?}",
            multi.err()
        );
    }

    #[test]
    fn source_admission_preserves_nested_selected_identity() {
        let source = "within Demo; package Box model Part output Real x=1; end Part; end Box;";
        let mut compiler = admission_fixture("Demo", source);
        let single = compiler.compile_str("Demo.Box.Part", source, "selected.mo");
        let multi = compiler.compile_str_multi("Demo.Box.Part", source, "selected.mo", &[]);
        assert!(
            single.is_ok(),
            "nested identity must compile: {:?}",
            single.err()
        );
        assert!(
            multi.is_ok(),
            "nested identity must compile: {:?}",
            multi.err()
        );
    }

    #[test]
    fn source_admission_rejects_changed_owned_extra() {
        let original = "within Demo; model Part output Real x=1; end Part;";
        let changed = "within Demo; model Part output Real x=2; end Part;";
        let mut compiler = admission_fixture("Demo", original);
        let result = compiler.compile_str_multi(
            "Probe",
            "model Probe Demo.Part part; end Probe;",
            "probe.mo",
            &[("edited.mo".into(), changed.into())],
        );
        assert!(result.is_err(), "changed extra must not be discarded");
        assert!(result.unwrap_err().contains("already owned"));
    }

    #[test]
    fn source_admission_preserves_quoted_root_identity() {
        let source = "within 'Demo.Root'; model Part output Real x=1; end Part;";
        let mut compiler = admission_fixture("'Demo.Root'", source);
        assert_eq!(
            compiler.source_root_namespaces("inline:admission"),
            vec!["'Demo.Root'"]
        );
        let single = compiler.compile_str("'Demo.Root'.Part", source, "selected.mo");
        let multi = compiler.compile_str_multi("'Demo.Root'.Part", source, "selected.mo", &[]);
        assert!(
            single.is_ok(),
            "quoted root must compile: {:?}",
            single.err()
        );
        assert!(multi.is_ok(), "quoted root must compile: {:?}", multi.err());
    }

    #[test]
    fn source_admission_rejects_conflicting_document_uri() {
        let source = "model Probe output Real x=1; end Probe;";
        let mut compiler = ModelicaCompiler::new();
        let error = compiler
            .compile_str_multi(
                "Probe",
                source,
                "probe.mo",
                &[(
                    "probe.mo".into(),
                    "model Probe output Real x=2; end Probe;".into(),
                )],
            )
            .unwrap_err();
        assert!(error.contains("conflicting contents"), "{error}");
        assert!(compiler.seated_user_uris.is_empty());
        assert!(
            compiler
                .compile_str_multi(
                    "Probe",
                    source,
                    "probe.mo",
                    &[("probe.mo".into(), source.into())]
                )
                .is_ok()
        );
    }

    #[test]
    fn admitted_library_revision_is_order_independent_and_content_sensitive() {
        let first = vec![
            ("b.mo".to_string(), "model B end B;".to_string()),
            ("a.mo".to_string(), "model A end A;".to_string()),
        ];
        let mut reordered = first.clone();
        reordered.reverse();

        assert_eq!(
            source_set_revision("demo", &first),
            source_set_revision("demo", &reordered),
            "source admission revision must not depend on filesystem enumeration order"
        );
        assert_ne!(
            source_set_revision("demo", &first),
            source_set_revision(
                "demo",
                &[("a.mo".to_string(), "model A end A; changed".to_string())]
            ),
            "a source edit must invalidate prepared FMI components"
        );

        let mut compiler = ModelicaCompiler::new();
        let empty_revision = compiler.library_revision();
        let report = compiler.load_source_root_in_memory("demo", "test-root", first);
        assert!(report.diagnostics.is_empty(), "{report:?}");
        assert_ne!(
            empty_revision,
            compiler.library_revision(),
            "successful source-root admission must update the compiler revision"
        );
    }

    /// Trivial smoke test — compile a self-contained model with no
    /// source library references, verifying the source-root-independent path.
    #[test]
    fn bare_model_compiles_without_library() {
        let src = r#"
            model Bare
              Real x(start=1);
            equation
              der(x) = -x;
            end Bare;
        "#;
        let mut c = ModelicaCompiler::new();
        let r = c
            .compile_str("Bare", src, "Bare.mo")
            .expect("bare model must compile without source library");
        // Just assert we got a DAE at all — shape details vary
        // by rumoca version.
        let _ = r.dae;
    }
    #[test]
    fn compiled_source_identity_uses_actual_participants_and_portable_inputs() {
        let source =
            "model IdentityProbe Real x(start=1,fixed=true); equation der(x)=0; end IdentityProbe;";
        let mut compiler = ModelicaCompiler::new();
        let unused = PreparedSourceRoot::prepare(
            "unused-root",
            "unused-root",
            vec![(
                "Unused.mo".into(),
                "package Unused model Part Real y; equation y=1; end Part; end Unused;".into(),
            )],
            Vec::new(),
        );
        assert!(compiler.install_source_root(unused).diagnostics.is_empty());
        let first = compiler
            .compile_str_multi(
                "IdentityProbe",
                source,
                "/host-a/editor.mo",
                &[(
                    "Ignored.mo".into(),
                    "model Ignored Real y; equation y=9; end Ignored;".into(),
                )],
            )
            .expect("inline compile");
        assert_eq!(
            first
                .source_closure
                .as_ref()
                .expect("strict closure")
                .target,
            "IdentityProbe"
        );
        assert_eq!(
            first
                .source_closure
                .as_ref()
                .expect("strict closure")
                .files
                .len(),
            1
        );
        let identity = compiler
            .compiled_source_content_identity(&first)
            .expect("unused source root is excluded");
        compiler.clear_user_documents();
        let second = compiler
            .compile_str("IdentityProbe", source, "/host-b/editor.mo")
            .expect("portable target");
        assert_eq!(
            identity,
            compiler
                .compiled_source_content_identity(&second)
                .expect("portable identity")
        );
        let changed = source.replace("start=1", "start=2");
        let third = compiler
            .compile_str("IdentityProbe", &changed, "/host-b/editor.mo")
            .expect("changed source");
        assert_ne!(
            identity,
            compiler
                .compiled_source_content_identity(&third)
                .expect("changed identity")
        );
        let required = "model RequiredProbe Unused.Part part; end RequiredProbe;";
        let compiled = compiler
            .compile_str("RequiredProbe", required, "RequiredProbe.mo")
            .expect("source-bearing dependency compiles");
        let dependent_identity = compiler
            .compiled_source_content_identity(&compiled)
            .expect("participating source bytes retain provenance");
        assert_ne!(identity, dependent_identity);
        assert_eq!(
            compiled
                .source_closure
                .as_ref()
                .expect("strict closure")
                .files
                .len(),
            2
        );
    }
}

#[cfg(test)]
mod compile_diagnostic_receipts {
    use super::*;

    #[test]
    fn failed_compile_retains_original_locations_without_seated_sources() {
        let mut compiler = ModelicaCompiler::new();
        let source = "model Broken\n Real x;\nequation\n x = missing;\nend Broken;";
        assert!(compiler.compile_str("Broken", source, "broken.mo").is_err());
        compiler.clear_user_documents();
        let diagnostics = compiler.take_compile_diagnostics("Broken", "broken.mo");
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains("missing")
                    && diagnostic.line == Some(4)
                    && diagnostic.col.is_some()
            }),
            "original failure must retain its source location: {diagnostics:?}"
        );
        assert!(
            compiler
                .take_compile_diagnostics("Broken", "broken.mo")
                .is_empty()
        );
    }
}
