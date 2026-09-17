//! Typed seam between the Modelica core and an optional execution transport.
//!
//! The compiler/document package must not depend on the solver worker package:
//! doing so makes every source/index consumer rebuild the simulation stack. The
//! core therefore owns only the data exchanged by the wasm parser bridge and
//! the callback table used by the execution package to install its transport.

use bevy::prelude::Resource;
use lunco_doc::DocumentId;
use rumoca_compile::parsing::StoredDefinition;

/// A parsed document returned by an off-thread Modelica parser.
#[derive(Debug)]
pub struct WorkerParseDone {
    /// Document whose source was parsed.
    pub doc_id: DocumentId,
    /// Source generation used for the parse.
    pub generation: u64,
    /// Best-effort parsed definition.
    pub ast: StoredDefinition,
    /// Recovery diagnostics produced by the parser.
    pub errors: Vec<lunco_doc::Diagnostic>,
}

/// A terminal failure returned by an off-thread Modelica parser.
#[derive(Debug)]
pub struct WorkerParseFailed {
    /// Document whose source was parsed.
    pub doc_id: DocumentId,
    /// Source generation used for the parse.
    pub generation: u64,
    /// Human-readable failure.
    pub error: String,
}

fn unavailable_parse(
    _doc_id: DocumentId,
    _generation: u64,
    _uri: String,
    _source: String,
) -> Result<(), String> {
    Err("Modelica execution transport is not installed".to_string())
}

fn unavailable_parse_done() -> Option<WorkerParseDone> {
    None
}

fn unavailable_parse_failed() -> Option<WorkerParseFailed> {
    None
}

fn unavailable_prewarm() {}

fn unavailable_install_library(_compressed: &[u8]) -> usize {
    0
}

fn unavailable_load_library_index(_compressed: &[u8]) -> usize {
    0
}

fn unavailable_reset_pipeline() {}

fn unavailable_fail_pipeline(error: String) {
    bevy::log::error!("[modelica] execution transport unavailable: {error}");
}

/// Callback table installed by [`lunco_modelica_execution`].
///
/// This keeps the dependency direction one-way: the compiler/document core
/// exposes the seam, while the native/wasm execution package supplies the
/// implementation. The default table is explicit failure, never a parser or
/// solver fallback.
#[derive(Resource, Clone, Copy)]
pub struct ModelicaWorkerBridge {
    dispatch_parse: fn(DocumentId, u64, String, String) -> Result<(), String>,
    try_recv_parse_done: fn() -> Option<WorkerParseDone>,
    try_recv_parse_failed: fn() -> Option<WorkerParseFailed>,
    prewarm: fn(),
    install_library_compressed: fn(&[u8]) -> usize,
    load_library_index: fn(&[u8]) -> usize,
    reset_pipeline: fn(),
    fail_pipeline: fn(String),
}

impl Default for ModelicaWorkerBridge {
    fn default() -> Self {
        Self {
            dispatch_parse: unavailable_parse,
            try_recv_parse_done: unavailable_parse_done,
            try_recv_parse_failed: unavailable_parse_failed,
            prewarm: unavailable_prewarm,
            install_library_compressed: unavailable_install_library,
            load_library_index: unavailable_load_library_index,
            reset_pipeline: unavailable_reset_pipeline,
            fail_pipeline: unavailable_fail_pipeline,
        }
    }
}

impl ModelicaWorkerBridge {
    /// Build a bridge from the execution package's typed callbacks.
    pub fn new(
        dispatch_parse: fn(DocumentId, u64, String, String) -> Result<(), String>,
        try_recv_parse_done: fn() -> Option<WorkerParseDone>,
        try_recv_parse_failed: fn() -> Option<WorkerParseFailed>,
        prewarm: fn(),
        install_library_compressed: fn(&[u8]) -> usize,
        load_library_index: fn(&[u8]) -> usize,
        reset_pipeline: fn(),
        fail_pipeline: fn(String),
    ) -> Self {
        Self {
            dispatch_parse,
            try_recv_parse_done,
            try_recv_parse_failed,
            prewarm,
            install_library_compressed,
            load_library_index,
            reset_pipeline,
            fail_pipeline,
        }
    }

    /// Dispatch one document parse to the installed execution transport.
    pub fn dispatch_parse(
        &self,
        doc_id: DocumentId,
        generation: u64,
        uri: String,
        source: String,
    ) -> Result<(), String> {
        (self.dispatch_parse)(doc_id, generation, uri, source)
    }

    /// Receive one completed document parse, if available.
    pub fn try_recv_parse_done(&self) -> Option<WorkerParseDone> {
        (self.try_recv_parse_done)()
    }

    /// Receive one failed document parse, if available.
    pub fn try_recv_parse_failed(&self) -> Option<WorkerParseFailed> {
        (self.try_recv_parse_failed)()
    }

    /// Ask the execution transport to prewarm its worker pool.
    pub fn prewarm(&self) {
        (self.prewarm)();
    }

    /// Install a compressed parsed-library artifact in the execution worker.
    pub fn install_library_compressed(&self, compressed: &[u8]) -> usize {
        (self.install_library_compressed)(compressed)
    }

    /// Install the compressed editor-index artifact in the execution worker.
    pub fn load_library_index(&self, compressed: &[u8]) -> usize {
        (self.load_library_index)(compressed)
    }

    /// Reset the execution transport before a new explicit library install.
    pub fn reset_pipeline(&self) {
        (self.reset_pipeline)();
    }

    /// Publish a terminal transport failure.
    pub fn fail_pipeline(&self, error: String) {
        (self.fail_pipeline)(error);
    }
}
