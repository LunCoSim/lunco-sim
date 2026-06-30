# 13 — Twin and Workflow

> Status: Active · Audience: contributors working on Twins, persistence, and workflow (§3a is flagged aspirational inline)
>
> **Twin** is LunCoSim's top-level persistent artifact: a folder of related
> domain-standard files plus a tool-specific manifest. This doc defines
> what's in a Twin, how it loads/saves, how users work inside and outside
> a Twin, and how cross-document references survive edits.

## Contents

- [1. What a Twin is](#1-what-a-twin-is)
- [1a. Three modes: File, Folder, Twin](#1a-three-modes-file-folder-twin)
- [1b. What's in a Twin — Documents, file references, endpoints](#1b-whats-in-a-twin--documents-file-references-endpoints)
- [2. Structure inside a Twin — flexible](#2-structure-inside-a-twin--flexible)
- [3. The `twin.toml` manifest](#3-the-twintoml-manifest)
- [3a. Twin is the simulation control surface — ASPIRATIONAL (not yet built)](#3a-twin-is-the-simulation-control-surface--aspirational-not-yet-built)
- [4. Orphan documents — working outside a Twin](#4-orphan-documents--working-outside-a-twin)
- [5. Session state — separate from Twin content](#5-session-state--separate-from-twin-content)
- [5a. Journal — `lunco-twin-journal` subsystem](#5a-journal--lunco-twin-journal-subsystem)
- [6. Startup flow](#6-startup-flow)
- [7. Creating a new Document — unified across types](#7-creating-a-new-document--unified-across-types)
- [8. Save, load, move, rename](#8-save-load-move-rename)
- [9. Reference strategies — the "hybrid" default](#9-reference-strategies--the-hybrid-default)
- [10. External libraries](#10-external-libraries)
- [11. Git and collaboration edge cases](#11-git-and-collaboration-edge-cases)
- [12. App composition and startup](#12-app-composition-and-startup)
- [13. Future: live-collab Twins](#13-future-live-collab-twins)
- [14. See also](#14-see-also)

## 1. What a Twin is

A **Twin** is a directory containing the full persistent state of one
simulated system — a lunar base, a rover test rig, a satellite
constellation, an electrical subsystem study. On disk:

```
my_lunar_base/              ← a Twin
├── twin.toml               ← tool manifest (the only file required to BE a Twin)
├── ... (domain-standard files — structure is flexible)
└── .lunco/                 ← session / workspace state (gitignored)
```

### The two-file strategy

A Twin separates **tool configuration** from **system modeling** into
different file formats. Each format does what it's best at:

| File kind | Format | Owns | Interop |
|-----------|--------|------|---------|
| Tool manifest | TOML (`twin.toml`) | Paths, UI preferences, reference strategy, environment selection | None expected — tool-specific |
| System structure | SysML v2 (`.sysml`) | Parts, ports, connections, requirements, verifications | ✅ Full, via any SysML v2 tool (Cameo, OpenMBEE, etc.) |
| Behavior | Modelica (`.mo`) | Equations, dynamics, parameters | ✅ Full, via Dymola, OMEdit, OpenModelica |
| Geometry | USD (`.usda` / `.usdc`) | Scenes, meshes, materials, transforms | ✅ Full, via Omniverse, Blender, USDView |
| Missions | RON/YAML (`.mission.ron`) | Timeline, events, maneuvers | Custom for now |

**Interop principle:** Each domain-standard file MUST contain only that
domain's standard syntax. No LunCoSim-specific annotations inside `.sysml`,
`.mo`, or `.usda` files. Tool-specific configuration lives in `twin.toml`
(or per-user state under `.lunco/`). This guarantees lossless round-trips
with external domain tools — a `system.sysml` opens cleanly in Cameo
because it's pure SysML v2, not SysML-with-LunCoSim-dialect.

The Twin manifest (`twin.toml`) is like `.vscode/settings.json`,
`Cargo.toml`, `.unity` project files, or `.git/config` — tool-specific
by design, not expected to round-trip through external tools. This is
the standard separation used by every mature engineering / creative
software stack.

Conceptually: a Twin is to LunCoSim what a project is to an IDE, a scene
is to a game engine, a stage is to Omniverse. It's the **unit of
saving, versioning, sharing, and opening**.

The word *Twin* reflects the "digital twin" ethos in `principles.md`: a
Twin is the best LunCoSim can reconstruct of a real (or designed) system.

### Twins compose — recursively

A Twin may embed other Twins via `[[children]]` in `twin.toml`.
Subsystems that are themselves complete digital twins (an Engine, a
Comm stack, a Science Payload) nest naturally under their parent
(Rover). This mirrors Cargo's workspace-with-packages model and SysML
v2's `part` decomposition:

```toml
# rover-twin/twin.toml
name = "Rover"

[[children]]
name = "engine"
path = "engine"                         # → rover-twin/engine/twin.toml

[[children]]
name = "shared-sensors"
url = "https://twins.lunco.space/sensors"   # remote ref (future)
```

Opening a Twin eagerly loads every local child. Remote URL children
are listed on the manifest but not followed today — reserved for the
remote-twin milestone. Cycles are guarded; missing child folders
tolerated with a warning.

A Workspace (the editor-session layer — see
[`01-ontology.md`](01-ontology.md) § 4e) can have multiple unrelated
root Twins open at once, each with its own recursive tree. That's how
users compose views across projects that live in different folders on
disk.

## 1a. Three modes: File, Folder, Twin

LunCoSim mirrors VS Code's tri-modal model. Users opt into progressively
more structured experiences without ceremony:

| Mode | What you open | What you get | What you don't |
|------|---------------|--------------|----------------|
| **Single File** (orphan) | `balloon.mo`, `scene.usda`, ... | Editor for that one file. Save in place. | No library / file tree. No cross-document tooling. |
| **Folder** (VS Code–like) | `my_models/` (no `twin.toml`) | File browser for the folder. Edit any file in tabs. | No Twin-level manifest. No registered external libraries. No automatic cross-reference rewriting. |
| **Twin** (full project) | `my_base/` with `twin.toml` | Library browser with categorization, cross-reference tracking, app-aware rename with preview, autosave, external library linking, workspace presets, environment config, reference-repair flow. | *(the full experience)* |

| VS Code analog | LunCoSim mode |
|----------------|---------------|
| Open File | Single File (orphan) |
| Open Folder | Folder (no `twin.toml`) |
| Open Workspace (`.code-workspace`) | Open Twin (`twin.toml`) |

**Promotion is always one click.** File → **Save as Twin...** upgrades
the current orphan or folder into a full Twin by writing a `twin.toml`
manifest and registering content. No migration required — the files
don't move.

## 1b. What's in a Twin — Documents, file references, endpoints

Following Unix convention, **every file in a Twin is just a file**.
We classify them by *how they're edited*, not by where they live:

| Kind | Editable inside LunCoSim? | How user edits | Examples |
|------|---------------------------|----------------|----------|
| **Document** | Yes — typed ops, undo/redo | LunCoSim panels (CodeEditor, Diagram, ParameterInspector, ...) or external domain tool | `*.mo`, `*.usda`, `*.sysml`, `*.mission.ron` |
| **File reference** | No — opaque container | External tool only (Photoshop, Blender, a text editor) | `*.png`, `*.glb`, `*.wav`, `*.pdf`, `*.md` |
| **Endpoint** *(future)* | N/A — remote, live | Connection config in `twin.toml`; not stored in Twin | FMI slave URL, telemetry stream, Nucleus server |

Most files the user creates during normal engineering work (models,
scenes, missions, requirements) are Documents. Assets referenced by
those Documents (textures on a material, mesh for a prim, PDF in
requirements) are file references — the Twin tracks them for
dependency listing and broken-reference detection, but LunCoSim does
not expose ops for editing them. Open a PNG in your image editor of
choice; save it; the Twin picks up the change.

Endpoints — live resources the Twin *references* but doesn't contain
— are an explicit future concern (see `10-document-system.md` § 2a).
We name the category now so we know not to stretch "Document" to
cover it; the abstraction is deferred until FMI and Nucleus land and
we can ground it in two concrete examples.

### `DocumentKindRegistry` — plugin-driven kind classification

The set of registered Document kinds is **open**, not a closed enum.
Each domain crate registers its own kind on plugin `build()` —
matches the same plugin-driven pattern used by `UriRegistry` (URIs)
and `BackendRegistry` (cosim backends). The registry lives in
`lunco-twin` behind an optional `bevy` feature so headless callers
(CLI tools, batch pipelines) keep zero-Bevy footprint.

```rust
// In a domain plugin's build()
let mut registry = app
    .world_mut()
    .resource_mut::<DocumentKindRegistry>();
registry.register(
    DocumentKindId::new("modelica"),
    DocumentKindMeta {
        display_name: "Modelica Model".into(),
        extensions: vec!["mo"],
        can_create_new: true,
        default_filename: Some("NewModel.mo"),
        uri_scheme: Some("modelica"),
        manifest_section: Some("modelica"),
    },
);
```

Consumers iterate the registry (File → New menu, `OpenFile`
classification, Twin panel's per-domain `BrowserSection`s,
`twin.toml` section dispatch) rather than matching a fixed enum. Adding a new
domain (USD, SysML, Julia, ...) is one new crate registering itself
— **no central edits** to `lunco-twin`, `lunco-workbench`, or any
other domain crate. Matches AGENTS.md §4 "Hotswappable Plugins"
mandate.

## 2. Structure inside a Twin — flexible

There is **no required folder structure** inside a Twin. Documents can
live wherever the user wants:

```
my_lunar_base/
├── twin.toml                       ← tool config (LunCoSim-specific)
├── system.sysml                    ← SysML system structure + requirements (standard)
├── requirements.sysml              ← more SysML (user's choice to split files)
├── balloon.mo                      ← Modelica at top level, fine
├── electrical/
│   ├── package.mo                  ← Modelica package (user's choice)
│   ├── battery.mo
│   └── solar_array.mo
├── thermal/
│   └── radiator.mo
├── main_scene.usda                 ← USD scene at top level, fine
├── scenes/
│   └── habitat_interior.usda
├── missions/
│   └── day1.mission.ron
├── connections/
│   └── power_grid.ron
└── assets/
    └── textures/
```

The app scans for Documents dynamically based on manifest-declared paths.
Users organize their Twin however matches their mental model: by system
(`electrical/`, `thermal/`), by lifecycle phase (`concept/`, `detail/`),
by vendor (`rover_supplier/`), or flat.

This matches how Dymola, Unity, Unreal, IntelliJ, and Omniverse treat
project content. Rigid folder layouts fight user intuition.

## 3. The `twin.toml` manifest

The only file a Twin *must* have. It declares:

```toml
[project]
name = "Lunar Base Alpha"
version = "0.3.0"
description = "South pole colony with ISRU plant"
lunco_version = ">=0.6"

[sysml]
# Entry-point SysML file for system structure + requirements.
# Additional `.sysml` files are picked up recursively from `paths`.
root = "system.sysml"
paths = ["."]

[modelica]
# Recursive scan from twin root for `.mo` files and `package.mo` markers.
# Matches MODELICAPATH behavior.
paths = ["."]

# External libraries (optional).
externals = [
    { name = "MSL", path = "@bundled:msl" },          # the bundled MSL
    { name = "MyPkg", path = "../shared_library/" },  # sibling folder
    { name = "Partners", path = "/opt/partner-lib/" }, # absolute
]

[usd]
# The Twin's entry-point USD stage — the one that becomes the active stage
# (projected into the Grid) when the Twin opens. Path is relative to the Twin
# root. This is the declared "which scene opens" answer that core USD leaves to
# convention; LunCoSim never *infers* a starting scene from a folder of many
# `.usda` files. Other `.usda` files in the Twin are a
# referenceable asset library, not auto-loaded. Full resolution rule (incl. the
# no-manifest folder fallback) in 21-domain-usd.md § "Which stage opens".
default_scene = "main_scene.usda"

[environment]
default = "moon"                  # or path to environment.ron

[references]
mode = "hybrid"                   # "path" | "uuid" | "hybrid" (default)

[workspace]
default = "build"                 # which workspace opens by default
```

Minimal Twin = just `[project]` + `[modelica]` (or whichever domains are
used). Everything else has sensible defaults.

## 3a. Twin is the simulation control surface — ASPIRATIONAL (not yet built)

> **Status: design / not implemented.** Today the Twin is a plain filesystem
> container (a folder + `twin.toml` manifest, owned through `lunco-twin` /
> `lunco-workspace`). The live-`Resource` control plane and the `TwinCommand`
> queue described below **do not exist in code** — there is no `TwinCommand`
> type. Control actions currently flow through the existing command/API fabric
> (`CommandMessage`, the HTTP `/api/commands` endpoint, cosim/run systems), not
> through a Twin-owned queue. This section is the target design.

Twin is not only an on-disk manifest — at runtime it would be a **live Bevy
`Resource`** that owns the simulation control plane for its slice of
the world. Full design in
[`14-simulation-layers.md`](14-simulation-layers.md). Key points that
would affect Twin authoring:

- `twin.toml` grows a `[scenarios.*]` section declaring named
  simulation graphs; scenario files live under `<twin>/scenarios/`.
- Past simulation sessions archive under `<twin>/runs/<id>/` with
  `run.toml` + `trace.mcap` + `checkpoints/` + `verdict.toml`.
- Every control action (start / pause / reset / step / warp /
  switch-fidelity / set-input / tunable-param edit) is a
  `TwinCommand` the Twin resource dispatches. Local UI, HTTP / gRPC
  remote, scripts, and replay all push to the same command queue.
- Headless = the same Twin resource in a Bevy app without rendering,
  driven by the HTTP adapter. Server-authoritative cosim per spec 022
  uses exactly this — clients attach replica backends and mirror state
  via network replication.

The Twin on disk is the source of truth; the runtime Twin resource is
the controller that mounts that truth into live simulation state.

## 4. Orphan documents — working outside a Twin

Users can open any Document without creating a Twin:

```sh
$ lunica balloon.mo
$ sandbox scene.usda
```

The Document opens in a single-document workspace. Same editing, same
save, same tabs. What's different:

| Aspect | Twin-hosted | Orphan |
|--------|-------------|--------|
| Library / file tree shows | All docs in Twin | Just this one (and optionally its folder) |
| Cross-document references | Resolved via Twin paths | Must be absolute or relative to the file |
| Save | Autosaves to known location | Prompts for location first time |
| Title bar | `LunCoSim — MyBase (Twin)` | `LunCoSim — balloon.mo` |
| Recent menu | Under "Recent Twins" | Under "Recent Files" |

**Promotion path:** File → **Save as Twin...** creates a `twin.toml` in
a chosen location, moves the current orphan into a sensible default
location (or asks), and reopens the document as part of the new Twin.

No automatic promotion prompts. Power users want orphan mode to stay
quiet; the "Save as Twin" action is always one menu click away.

## 5. Session state — separate from Twin content

The per-app working state lives in `<twin>/.lunco/session.toml`
(gitignored by default) when a Twin is open, or in the user's config
when working with orphans:

- Which Documents are open in tabs
- Panel visibility + dock positions per workspace
- Active workspace
- Cursor / scroll / zoom / view states
- Recent view-camera poses

Session state is **not** Twin content — two users collaborating on the
same Twin can have wildly different session state. Teams who *want* to
share layouts (e.g., "everyone gets the same default panel layout")
can commit `.lunco/` or author layout defaults in `twin.toml`.

Closing and reopening the app restores the session exactly.

## 5a. Journal — `lunco-twin-journal` subsystem

The Twin's journal is a **dedicated subsystem**, not an in-memory side
effect of the UI. It lives in its own crate, `lunco-twin-journal`, and
captures *every observable event* in the Twin's lifecycle: structured
edits, compiles, simulation runs, parameter scans, bookmarks, agent
queries, comments, screenshots, file imports — anything a future
reviewer might want to replay or audit.

The journal is not "edit history" in disguise. It's the engineering
record of the Twin — closer in spirit to a flight recorder or a
laboratory notebook than to git. Documents are *what* the Twin
contains; the journal is *what happened to it*.

### Why a separate crate

| Responsibility | Crate |
|----------------|-------|
| `twin.toml`, document set, file references, paths | `lunco-twin` |
| Append-only event log: storage, schema, query, replay | `lunco-twin-journal` |
| Per-domain ops (`ModelicaOp`, `UsdOp`, …) | domain crates |
| UI panel that *displays* the journal | `lunco-modelica` (and per-domain UI) |

Splitting the journal out keeps `lunco-twin` focused on file-set
management and lets headless tooling (CLI exporters, CI pipelines,
audit scripts) depend on the journal without pulling in the Bevy /
egui surface area. Domain crates **never write directly to journal
files** — they emit `JournalEvent`s through a Bevy observer chain;
`lunco-twin-journal` owns the only writer.

### Storage

```
my_twin/
├─ twin.toml
├─ models/Engine.mo
└─ .lunco/
   └─ journal/
      ├─ index.toml          # active segment, schema version, retention
      ├─ 00000-2026-04-29.ndjson   # append-only segments
      ├─ 00001-2026-04-30.ndjson
      └─ blobs/                    # large payloads (screenshots, sim
          └─ d3a7c…/                # results, plot exports) keyed by hash;
              └─ 00.png             # entry references via "blob": "d3a7c…"
```

- **Append-only NDJSON segments**, one event per line. Segments rotate
  by date or by size cap (default ~1 MB) to stay git-friendly. Old
  segments are never rewritten.
- **Blob sidecar** holds anything too big or too binary for NDJSON —
  screenshots, FMU result CSVs, exported plots. Entries reference
  blobs by content hash; deduplication is automatic.
- **VCS by default**: `journal/index.toml` and `journal/*.ndjson` are
  committed; `journal/blobs/` is opt-in via
  `twin.toml [journal] commit_blobs = true` (most teams want LFS or
  out-of-tree storage for binary payloads).
- Retention defaults to "keep all"; explicit prune via
  `twin.toml [journal] retain_days = N`.

### Schema — one event

```rust
pub struct JournalEvent {
    pub ts: DateTime<Utc>,            // monotonic in segment order
    pub session: SessionId,           // opaque per-app-launch id
    pub actor: ActorId,               // git user.name | env | "agent:<name>"
    pub source: EventSource,          // canvas | editor | api | undo | redo
                                      //  | compile | sim | import | comment
    pub kind: EventKind,              // (typed enum, see below)
    pub payload: serde_json::Value,   // domain-specific, schema'd per kind
    pub blob: Option<BlobRef>,        // optional content-hash reference
    pub note: Option<String>,         // optional human-authored reason
}

pub enum EventKind {
    // Document mutations (covers all `ModelicaChange` etc.)
    DocumentEdited { doc: TwinPath, gen: u64, op: String },
    // Compile / simulate lifecycle
    CompileStarted { doc: TwinPath, class: String },
    CompileSucceeded { doc: TwinPath, class: String, duration_ms: u64 },
    CompileFailed   { doc: TwinPath, class: String, error: String },
    SimRunStarted   { doc: TwinPath, class: String, params: BTreeMap<String, f64> },
    SimRunFinished  { doc: TwinPath, blob: BlobRef /* result CSV */ },
    // Curation
    Bookmark        { label: String, target: TwinTarget },
    Comment         { target: TwinTarget, body: String },
    Screenshot      { blob: BlobRef, target: TwinTarget },
    // External
    FileImported    { doc: TwinPath, source_uri: String },
    AgentQueried    { agent: String, query: String, response: String },
}
```

`EventKind` is a typed Rust enum with `serde` (un)tagged JSON
representation. New variants are additive — older readers fall
through to a `JournalEvent::Unknown` branch and preserve the row
on rewrite, so a Twin opened in an old client can still be saved.

`TwinPath` is the Twin-relative path. `TwinTarget` carries enough
context to re-open exactly the same view (`{doc, class, component?,
range?}`).

### What the journal is **not**

- **Not the AST history.** The current AST lives in the Document file.
  The journal records the deltas; replay reconstructs them.
- **Not the undo stack.** Undo is bounded ring-buffer state in the
  open Document. The journal logs the original op AND its undo / redo
  as separate entries (with `source = undo|redo`) so a reviewer can
  audit *why* state moved backward.
- **Not session state.** Tab switches, panel resizes, scroll
  positions belong in `session.toml`.
- **Not telemetry to a vendor.** All journal data stays in the Twin.
  Anonymous telemetry, if it ever exists, is a separate opt-in
  channel.

### Crate surface (`lunco-twin-journal`)

```rust
// Storage
pub trait JournalStore {
    fn append(&mut self, event: JournalEvent) -> Result<()>;
    fn query(&self, q: &JournalQuery) -> Result<Vec<JournalEvent>>;
    fn store_blob(&mut self, bytes: &[u8]) -> Result<BlobRef>;
    fn read_blob(&self, blob: &BlobRef) -> Result<Vec<u8>>;
}
pub struct FsJournalStore { /* default impl over `<twin>/.lunco/journal/` */ }

// Bevy integration (behind a `bevy` feature flag)
pub struct JournalPlugin;          // wires the writer + observer fan-in
pub struct JournalAppend(pub JournalEvent);   // event/observer trigger
pub struct JournalQuery { /* doc? actor? kind? since? limit? */ }

// Replay / revert
pub trait JournalReplay {
    fn replay_to(&self, store: &dyn JournalStore, target: &JournalCursor)
        -> Result<TwinSnapshot>;
}
```

Domain crates fire `JournalAppend` events; the `JournalPlugin`
observer drains them through `JournalStore::append`. The store is
swappable — `FsJournalStore` for normal use, an in-memory store for
tests, an HTTP store for hosted Twins down the road.

### UI surface

The bottom-dock Journal panel is a *view* over `JournalStore::query`.
It filters by doc / actor / kind / source / time window, clicks an
entry to open the corresponding Twin target, and exposes
"Revert to here" (replay inverse edits onto a scratch copy) and
"Compare with file" (diff journal-implied state against the current
Document source — catches out-of-band edits).

### API surface

External agents (MCP, HTTP API) read the journal to ground their
work in prior context. They cannot append directly — only the Bevy
observer chain may write, ensuring journal ↔ document consistency.

```
journal.query     { doc?, kind?, actor?, since?, limit? } → Vec<JournalEvent>
journal.snapshot  { at: JournalCursor }                   → TwinSnapshot
journal.blob      { ref: BlobRef }                        → bytes
```

### Why this design

- **Reproducibility.** A reviewer six months later replays the
  journal to see exactly how the rocket model came to be: which
  parameters were swept, which compile errors were hit and how they
  were resolved, which simulation runs informed which design
  decisions.
- **AI-assisted workflows.** Agents read the journal to understand
  intent before mutating; their own actions are tagged
  `actor = "agent:<name>"` so humans can audit agent contributions
  separately.
- **Twin handoff.** Cloning a Twin clones its history. Merging two
  Twin branches merges NDJSON segments as plain text — monotonic
  timestamps make conflicts rare and resolvable.
- **Hardware-in-the-loop / ops integration.** Sim runs, telemetry
  snapshots, and live-feed events all land in the journal as first-
  class entries — the same surface that records "user added a wire"
  records "FMU run completed at t=45.2s with thrust=12.4kN".

The journal is conceptually a flight recorder for a digital twin —
flat, append-only, content-addressed, queryable. LunCoSim doesn't
reconstruct documents from the journal; the journal records what
happened so anyone (human or agent, today or later) can answer
*why is this Twin in the state it's in?*

### Forward path: SysML v2 REST API backbone

`lunco-twin-journal` is also the substrate on which we'll mount a
**SysML v2 REST API** (OMG SysML v2 / KerML services spec). The
v2 spec is built around commits, branches, projects, elements, and
queries — exactly the abstractions the journal already supplies, just
under different names:

| SysML v2 concept | LunCoSim mapping |
|------------------|------------------|
| `Project` | a Twin |
| `Branch` | a git branch over the Twin (or a journal cursor range) |
| `Commit` | a `JournalCursor` — append-only, content-hashed range of events terminated at a logical boundary (compile success, save-all, explicit "Tag commit") |
| `Element` (`Part`, `Connection`, `Action`, …) | a Twin target (`TwinPath` + class + component path) |
| `RecordedRelationship` / `RelationshipUsage` | `EventKind::DocumentEdited { op: AddConnection }` and friends |
| `Query` over commit / element | `JournalStore::query` + `JournalReplay::replay_to` |
| Element-level provenance (`createdBy`, `modifiedBy`) | `JournalEvent.{actor, ts, source}` |

Concretely:

- The trait `JournalStore` is shaped to satisfy SysML v2's `commits`
  / `elements` endpoint contracts: an immutable, content-addressed
  store with key-by-hash retrieval, append-only writes, range queries.
- `JournalReplay::replay_to(JournalCursor)` is the engine behind
  SysML v2 `GET /projects/{id}/commits/{commit}/elements` — it
  produces the element graph at the named commit by replaying events.
- `BlobRef` is content-addressed exactly like SysML v2's element
  identifiers (`@id` is hash-derivable from canonical event sequence).
- `EventKind` variants slot into the v2 metamodel:
  `DocumentEdited` → `RecordedChange`, `Comment` → `Comment`,
  `SimRunFinished` → `AnalysisCaseUsage` result attachment, etc.

This isn't a v1 deliverable — the SysML v2 REST spec is still
stabilising, and we're not chasing OMG conformance until it does.
But the journal's data model is **deliberately a strict superset**
of what SysML v2 requires. When we bolt on the REST adapter
(`lunco-twin-journal-sysml-v2` crate, target ~M3), it becomes a
translator from SysML v2 paths/verbs onto `JournalStore` + replay —
no new persistence layer, no schema migration, no forked domain
model.

The pragmatic order:

1. **Now** — `lunco-twin-journal` lands with `FsJournalStore`,
   `JournalPlugin`, the existing UI panel.
2. **Next** — domain crates fan their existing `*Change` events into
   `JournalAppend`; `JournalReplay` lands so "Revert to here" works.
3. **Later** — `lunco-twin-journal-sysml-v2` adds the OMG REST
   surface: `/projects`, `/commits`, `/branches`, `/elements`,
   `/queries`. Multi-Twin backends (cloud, hosted) plug in here as
   alternate `JournalStore` impls (S3, Postgres, KerML server).

The journal stops being a "nice audit log" and becomes the protocol
boundary — every external integration (a v2-conformant tool, a
Modelica master, an external sim runner, a downstream PLM/MBSE
system) reads and writes through it.

## 6. Startup flow

All three apps (`lunco-sandbox`, `luncosim`, `lunica`)
share the same logic:

```
App launches
    │
    ├── CLI arg given?
    │      │
    │      ├── Path is a Twin folder (has twin.toml) → Open Twin
    │      ├── Path is a known document type → Open as orphan
    │      ├── Path is a folder without twin.toml → Show "no Twin here"
    │      │   dialog: "Create a Twin here, or open a file inside?"
    │      └── Unknown type → error dialog, then Welcome Screen
    │
    └── No CLI arg?
           │
           ├── Setting "restore previous session" = on (default):
           │     Reopen last Twin OR last orphans
           │
           └── Otherwise: Welcome Screen
```

### Welcome Screen

Shown when nothing else is loaded. The central viewport area becomes a
styled panel — the rest of the workbench chrome (menu bar, activity bar,
status bar) remains visible so the user sees the *app*, not a splash.

```
┌───────────────────────────────────────────────────────────────┐
│                          LunCoSim                             │
│                  Digital Twin of the Solar System             │
│                                                               │
│  Start                                                        │
│    🆕  New Twin                                               │
│    📁  Open Twin                                              │
│    📄  Open File                                              │
│                                                               │
│  Recent                                                       │
│    🔭  my_lunar_base            (Twin, 2 hours ago)           │
│    🔭  mars_outpost             (Twin, yesterday)             │
│    📄  balloon.mo               (standalone, 3 days ago)      │
│                                                               │
│  Examples                                                     │
│    ⚡  Electrical Circuit (RLC)                               │
│    🏗️  Mechanical Spring-Mass                                 │
│    🚀  Simple Rover on Moon                                   │
│                                                               │
│  Learn                                                        │
│    📘  Documentation   💬  Community   🎓  Tutorials          │
└───────────────────────────────────────────────────────────────┘
```

The Examples list is **filtered per app** to what it knows how to open:

| App | Examples shown |
|-----|----------------|
| `lunica` | Modelica-only models |
| `lunco-sandbox` | 3D sandbox scenarios |
| `luncosim` | All examples |

## 7. Creating a new Document — unified across types

Single entry: File → New → <type>. Or Ctrl+Shift+N (keybind).

Dialog shape (Modelica shown, other domains analogous):

```
┌────────────────────────────────────────────┐
│ New Modelica Model                         │
├────────────────────────────────────────────┤
│ Name:     [NewBalloon          ]           │
│ Kind:     (●) model (○) block (○) connector
│                                            │
│ Template: (●) Empty                        │
│           (○) From MSL component...        │
│           (○) Copy from existing...        │
│                                            │
│ Location: ── In Twin: my_lunar_base ──     │
│           Folder: / ▼                      │
│           (or) ○ Standalone (pick location)│
│                                            │
│                      [ Cancel ] [ Create ] │
└────────────────────────────────────────────┘
```

- **If a Twin is open:** folder selector defaults to the Twin root; user
  can pick any subfolder. The new file lands there; it's automatically
  picked up by the Modelica scan path.
- **If no Twin:** the dialog's Location section shows "Standalone (will
  prompt for location on save)" and a hint: "To organize work, create a
  Twin first."

After confirm:
1. A `ModelicaDocument` is created in memory (buffer, no file yet)
2. The Analyze workspace activates
3. Diagram panel shows empty canvas; Code editor shows skeleton:
   ```modelica
   model NewBalloon
     // Parameters, variables, equations go here.
   end NewBalloon;
   ```
4. Title bar: `● NewBalloon [unsaved]`

On Ctrl+S:
- Twin-hosted: writes to the chosen location inside the Twin, Library
  Browser refreshes automatically.
- Orphan: shows "Save As" dialog, writes to chosen location.

Save is always silent when the location is known — no "Save?" dialogs
while editing.

### 7a. Drafts — multiple untitled Documents in memory

A Document created via File → New but not yet saved is a **Draft** — an
in-memory buffer with no disk path. Users can have many drafts open
simultaneously; each is an independent Document instance.

Tabs show draft status visibly: `● Untitled-1.mo` (dot = dirty, no path).
After save, the tab becomes `Balloon.mo` (no dot).

**Architecturally**, a Draft isn't a separate concept — it's a Document
with `DocumentLocation::Draft(DraftId)` instead of
`DocumentLocation::Disk(path)`. Every other layer (Op-based editing,
views, undo, autosave) works identically. Only the path-resolution layer
and the save flow know about the distinction.

#### Virtual filesystem for cross-references between drafts

LunCoSim's drafts can reference each other, unlike VS Code's untitled
buffers. This matters when a user drafts a multi-file design and wants
to *compile* or *simulate* before saving.

Drafts live at synthetic paths (e.g., `mem:///draft/<DraftId>.mo`). The
Modelica compile pipeline resolves these paths via an in-memory source
cache; USD references to draft paths resolve the same way. On save, the
synthetic path is replaced with the real path, and the normal
cross-reference rewrite flow (§ 8) updates every reference that pointed
at the draft.

```rust
// Sketch — in lunco-ui
pub enum DocumentLocation {
    Disk(PathBuf),
    Draft(DraftId),   // synthetic ID; resolves via in-memory buffer
}
```

#### Save All workflow

Menu: File → **Save All**. Behavior depends on context:

- **Twin open — drafts save into the Twin.** A dialog shows a table of
  drafts with proposed filenames and folders; user can edit each:
  ```
  Save 3 drafts to Twin: my_lunar_base

    ● Untitled-1.mo   → [ / ▼ ]            Battery
    ● Untitled-2.mo   → [ models/ ▼ ]      Motor
    ● Untitled-1.usda → [ scenes/ ▼ ]      Test

                                 [ Cancel ] [ Save All ]
  ```
  Cross-references between drafts are rewritten to the new real paths
  using the standard app-aware rename flow.

- **Folder open — drafts save into that folder.** Same table, defaults
  pointing at the open folder root.

- **No Twin, no Folder — user has several drafts.** Save All offers
  promotion:
  > You have 3 drafts. Creating a Twin will let you save them together
  > with full tooling. [ Create Twin... ] [ Save as standalone files... ]
  > [ Cancel ]

  This is the **natural promotion moment**: multi-draft work implies
  project-level intent. The app nudges toward Twin without forcing it.

#### Auto-recovery

Drafts autosave every few seconds to
`$XDG_DATA_HOME/lunco/drafts/` (or platform equivalent). On app start,
if drafts exist from a previous session:

> **3 unsaved drafts from your last session.** [ Recover ] [ Discard ]

No data loss from crashes. Matches VS Code's `Backups` behavior.

#### Closing vs. discarding a draft

- Clean draft (never touched after creation) → closes silently.
- Dirty draft → prompt: "Save changes to Untitled-1? [ Save ] [ Discard ] [ Cancel ]"

## 8. Save, load, move, rename

### Autosave

- **In a Twin:** autosave after ~5 s of idle. Matches VS Code / IntelliJ.
  Session state saves more aggressively.
- **Orphan:** autosave only after the first explicit save (so the user
  controls initial location). After that, same ~5 s idle rule.

### Move / rename through the app (preferred path)

Users rename or move Documents via the app's file panel (or
Command Palette > "Rename File"). When they do:

1. App writes the file to its new location.
2. App scans every Document in the Twin for references to the old path.
3. A **preview dialog** shows what would change:
   ```
   ┌─────────────────────────────────────────────────────────┐
   │ Rename: balloon.mo → airships/balloon.mo                │
   │                                                         │
   │ This rename will update 3 references:                   │
   │                                                         │
   │   main_scene.usda:                                      │
   │     /World/Balloon.modelicaModel: ./balloon.mo →        │
   │                                   ./airships/balloon.mo │
   │   ...                                                   │
   │                                                         │
   │ [ Cancel ]  [ Rewrite References ]                      │
   └─────────────────────────────────────────────────────────┘
   ```
4. On confirm: all affected Documents are updated and saved.

This is the standard refactor-aware rename pattern from IntelliJ and
VS Code.

### Move / rename through the OS or git

Inevitable — people use `mv`, `git mv`, or drag-drop between windows.
After the fact, the app detects orphaned references:

1. On next Twin load, scan finds references pointing at non-existent paths.
2. **Non-blocking banner** at the top of the workbench:
   > ⚠ **3 broken references**, last updated via external tool. [ Fix ]
3. Clicking "Fix" opens a repair dialog:
   - For each broken reference, suggest the nearest match (by filename,
     model name, content hash heuristic).
   - User accepts / rejects per reference or "Accept all high-confidence
     suggestions."
   - Updated references saved, banner dismissed.

The Twin remains fully usable while refs are broken. The user fixes at
their leisure; features that depend on a broken reference simply show an
error badge on the affected panel or entity.

## 9. Reference strategies — the "hybrid" default

References between Documents use domain-native mechanisms where they
exist, with UUID fallback for cross-domain:

### Within Modelica (model-to-model)

Modelica's native package system. References are fully-qualified names:

```modelica
import MyPkg.Electrical.Battery;
```

Resolved via Modelica path (`[modelica] paths` in `twin.toml`, plus
declared `externals`). Moving `Battery.mo` into a different package
folder updates its qualified name, which requires updating import
statements — handled by the app-aware rename flow.

### Within USD (prim-to-prim, sublayer refs)

USD-native — `@./other_scene.usda@` relative paths, or absolute paths via
Nucleus URIs. When the user moves a USD file via the app, all USD refs
get rewritten. Cross-stage prim references use `SdfPath`, which is
rename-stable within a stage.

### Cross-domain (Modelica ↔ USD ↔ Mission)

Stable UUIDs. For example, a `ModelicaAttachment` component on a USD
prim links the prim to a Modelica model by:

```rust
struct ModelicaAttachment {
    usd_prim_path: SdfPath,       // path, current location
    modelica_class: String,        // fully-qualified Modelica name
    link_id: Uuid,                 // stable across renames
}
```

A mission event targets a Space System by UUID, not by scene-path. When
the USD prim moves, the UUID finds the new path via a reverse-lookup
index maintained at Twin load.

### The `hybrid` mode in `twin.toml`

Default. *Within* a domain, use native references. *Across* domains,
use UUIDs. Covers 99% of use cases:

- Simple (references look like paths or Modelica names — familiar)
- Robust (critical cross-domain links survive renames)
- Tooling-friendly (git diffs are readable)

Other modes — `path` (everything relative) and `uuid` (everything stable-ID)
— exist for edge cases but aren't recommended defaults.

## 10. External libraries

`twin.toml`'s `[modelica] externals` declares additional search paths:

```toml
[modelica]
externals = [
    { name = "MSL", path = "@bundled:msl" },
    { name = "MyPkg", path = "../shared_library/" },
]
```

- **Bundled libraries** (`@bundled:msl`) ship with LunCoSim and are
  always available.
- **Relative paths** work great for sibling-folder libraries in a
  monorepo.
- **Absolute paths** are supported but break portability — warn on save.
- Future: **git URL** form (`@git:https://.../lib.git@v1.0`) — fetch
  on Twin load, like Cargo's git deps. Out of scope for v1.

Same pattern applies to USD (asset libraries) and future SysML
(block libraries).

## 11. Git and collaboration edge cases

A Twin is a normal folder, works with any VCS. Designed assumptions:

- `.lunco/` (session state) is gitignored by default; a generated
  `.gitignore` in new Twins handles this.
- A `.lunco/` entry being version-controlled (some teams want shared
  layouts) is supported — add `!.lunco/layouts.toml` to the Twin's
  `.gitignore` to opt in.
- `twin.toml` is always committed — it defines the Twin.
- Modelica `package.mo` files are committed — they define the package
  hierarchy.
- Binary assets (textures, meshes) use Git LFS or submodules per team
  preference; LunCoSim just reads files.

### Merge conflicts

Three categories:
- **Whole-file changes** (two people edit `balloon.mo`): handled by git
  merge like any text file. Modelica source is textual so conflicts are
  resolvable.
- **Reference rewrites** (user A moves `balloon.mo`, user B adds a
  reference to old path): appears as a broken reference after merge —
  fixed via the "Fix broken references" flow.
- **Structural conflicts** (two people add different components at the
  same position in a diagram): future Document System work —
  op-replay-based conflict resolution using the typed op stream, not
  textual diff.

## 12. App composition and startup

### Key insight: apps differentiate by plugins, not by hardcoded scenes

The three binaries — `lunica`, `lunco-sandbox`,
`luncosim` — share the **same Twin-loading machinery** from
`lunco-workbench` and `lunco-twin`. They differ only in:

1. **Which domain plugins they register** (what Document types the app
   can open and edit).
2. **Which Workspaces they enable by default**.
3. **Which examples their Welcome Screen surfaces**.

No app hardcodes a scene or a default file. The startup flow in § 6 runs
uniformly for all of them.

### Per-app plugin composition

| App | Default Workspace | Domain plugins registered | Welcome examples shown |
|-----|-------------------|---------------------------|------------------------|
| `lunica` | **Analyze** | `ModelicaPlugin` + `ModelicaInspectorPlugin` | Modelica examples only (circuit, spring-mass, thermal, …) |
| `lunco-sandbox` | **Build** | `CoSimPlugin`, `ModelicaCorePlugin`, `SandboxEditPlugin`, `EnvironmentPlugin`, `UsdPlugins`, `Mobility`, `Controller`, `Avatar`, … | Sandbox examples (rover-on-moon, balloon-test, …) |
| `luncosim` | **Build** (or last-used) | All of the above + `CelestialPlugin` + `LuncoUiPlugin` (MissionControl) | All examples, categorized |

A `lunco-workbench` config type (passed to `WorkbenchPlugin`) declares
which domains + workspaces this app supports. The workbench uses it to:

- Filter Welcome Screen examples
- Populate File → Open / New menus
- Enable / disable Workspaces
- Decide which file extensions are "known types" for the orphan-open path

### What replaces current `setup_sandbox` startup code

Today each binary has a `setup_sandbox` function that hardcodes scene
setup — spawning a Camera2d, reading `assets/models/Battery.mo`, inserting
a specific `ModelicaModel` component, etc. Under the new model, **all of
that goes away**. Startup belongs to `lunco-workbench`:

```rust
// lunica (before)
fn main() {
    app.add_plugins(DefaultPlugins)
       .add_plugins(EguiPlugin::default())
       .add_plugins(bevy_workbench::WorkbenchPlugin { ... })
       .add_plugins(ModelicaPlugin)
       .add_systems(Startup, setup_sandbox);   // hardcodes Battery.mo
}

// lunica (after)
fn main() {
    app.add_plugins(DefaultPlugins)
       .add_plugins(EguiPlugin::default())
       .add_plugins(lunco_workbench::WorkbenchPlugin::new()
           .workspace_default(Workspace::Analyze)
           .examples_dir("examples/modelica/"))
       .add_plugins(lunco_twin::TwinPlugin)
       .add_plugins(ModelicaPlugin);
    // No setup_sandbox. Workbench handles startup.
}
```

The old `setup_sandbox` logic — "load Battery.mo and spawn it" — becomes
a *ship-with-app example Twin* that the user can open from the Welcome
Screen. Examples live in `examples/modelica/<example_name>/` directories,
each with a `twin.toml` and its Documents.

### Per-app: "New file" menu entries

Each app exposes relevant `File → New →` items based on what it can edit:

| App | New menu items |
|-----|---------------|
| `lunica` | New Modelica Model, New Modelica Package |
| `lunco-sandbox` | New Scene (USD), New Twin, New Modelica Model |
| `luncosim` | New Scene, New Modelica Model, New Mission, New SysML Block, New Twin |

Across all three, the Command Palette can find any action — even if a
menu item isn't exposed. Power users get uniform access.

### No legacy coexistence

The migration from `bevy_workbench` to `lunco-workbench` is a **clean
cutover**, not a feature-flagged coexistence. Each domain migrates its
panels when ready; the final commit removes the `bevy_workbench`
dependency and the now-unused `setup_sandbox` functions in one pass.

We accept short periods during migration where a particular domain's
panels might look rough as they move to the new Panel trait, rather than
maintain two parallel UI stacks with flags to switch between them. The
reward is code that doesn't carry transitional scar tissue.

## 13. Future: live-collab Twins

Out of scope for initial implementation; designing the foundations now:

- Op streams (from [10-document-system.md](10-document-system.md)) are
  already serializable — they're the wire protocol for collab.
- UUIDs in hybrid reference mode mean cross-user renames don't break
  each other's references.
- Twin could be served by a Nucleus-tier server (or peer-to-peer CRDT)
  that broadcasts ops between clients.
- `twin.toml` could declare `[collab] server = "..."` to opt into live
  syncing.

A dedicated collab-roadmap doc is TBD; see [18-unified-journal-and-history.md](18-unified-journal-and-history.md) for the multi-author journal substrate.

## 14. See also

- [`00-overview.md`](00-overview.md) — three-tier architecture
- [`01-ontology.md`](01-ontology.md) — Document, DocumentOp, DocumentView definitions
- [`10-document-system.md`](10-document-system.md) — the editable-artifact pattern that lives inside a Twin
- [`11-workbench.md`](11-workbench.md) — the UI that hosts Twin editing
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica Document specifics
- [`21-domain-usd.md`](21-domain-usd.md) — USD Document specifics
