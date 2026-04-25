# Feature Specification: Model Source Listing & Unified Open

**Feature Branch**: `032-model-source-listing`
**Created**: 2026-04-25
**Status**: Draft
**Input**: Expose all openable model sources (Twin folder, bundled examples, MSL, ephemeral docs) and current workspace state through the API, with a single scheme-aware `Open` command.

## Agent-Workflow Anchor

The validation scenario for this spec is the start of an end-to-end agent-driven simulation: *"I want to simulate the Annotated Rocket Engine — load it, set the valve to 50%, then 100%, pause, resume."* This spec covers steps 1–2 of that workflow (**find** the model across all sources and **open** it through one URI) and the workspace-state visibility (**what is currently open**) that every subsequent step depends on. Live model interaction — describing a model's inputs, mutating runtime values, snapshotting variables — is deferred to spec 033 (`agent-driven-simulation`). The two specs together produce the agent loop; this one is the half that lets the agent *find and load*, and report *what is loaded right now*.

## Problem Statement

The workbench has four distinct sources a user can open content from — the open Twin folder, embedded `assets/models/*.mo` bundled examples, the Modelica Standard Library (~2500 classes), and in-memory Untitled docs — plus a live workspace state (open tabs, dirty flags, view modes). Each is reachable through the UI, but **none is enumerable through the API**. An AI agent driving the workbench via MCP today has to call `discover_schema`, then guess class names; there is no way to ask "what can I open?" or "what is currently open?". The three open commands (`OpenFile`, `OpenClass`, `OpenExample`) carry different semantics and the agent has to know which to call. Bundled examples in particular cannot be opened through the public API at all — `OpenFile` does raw `fs::read_to_string` and does not understand the `bundled://` URI scheme used internally by the Welcome tab. MSL load status is invisible: `MSL_LIBRARY` is prewarmed on a background thread but a caller has no way to ask whether prewarm finished, so a query during cold start blocks the API thread for hundreds of milliseconds.

## User Scenarios & Testing

### User Story 1 - List Bundled Examples (Priority: P1)

As an AI agent driving the workbench via MCP
I want to enumerate the embedded example models without knowing their filenames
So that I can offer the user a choice or pick one for a smoke test

**Why this priority**: Smallest surface, validates the architecture (query-style command returning structured data through the API). Bundled is finite (~9 entries), needs no pagination, lives in compile-time-embedded data.

**Independent Test**: Issue `list_bundled` over MCP, receive a JSON array with one entry per `.mo` file under `assets/models/`, each carrying `filename` and `tagline`.

**Acceptance Scenarios**:

1. **Given** the workbench is running, **When** I call `list_bundled`, **Then** I receive every `.mo` file from `assets/models/` with its tagline (extracted from the `// tagline:` header marker if present, empty otherwise).
2. **Given** a new `.mo` is dropped into `assets/models/` and the binary is rebuilt, **When** I call `list_bundled`, **Then** the new file appears without any code change to the listing endpoint.
3. **Given** the binary is built for `wasm32`, **When** I call `list_bundled`, **Then** results are identical to the desktop build (data is embedded, not filesystem-scanned).

---

### User Story 2 - List Open Documents With Workspace State (Priority: P1)

As an AI agent
I want to see every document currently open in the workspace, with its origin, dirty flag, view mode and active state
So that I can decide whether to focus an existing tab or open something new

**Why this priority**: Layer-2 question — what *is* on screen — distinct from the catalog endpoints. Drives every other decision the agent makes. Includes ephemeral docs that have no catalog entry.

**Independent Test**: Issue `list_open_documents`, receive a JSON array where each entry includes `doc_id`, `title`, `origin` (file path or untitled name), `kind`, `dirty`, `active`, `view_mode`.

**Acceptance Scenarios**:

1. **Given** I have opened `RC_Circuit.mo` from the bundled examples (which lands as an Untitled doc) and `Battery.mo` from a Twin folder, **When** I call `list_open_documents`, **Then** both appear with correct origins (`{kind:"untitled", name:"RC_CircuitCopy"}` and `{kind:"file", path:"<abs>", writable:true}`) and the active tab is flagged `active:true`.
2. **Given** I have edited an open document but not saved it, **When** I call `list_open_documents`, **Then** the corresponding entry has `dirty:true`.
3. **Given** no documents are open, **When** I call `list_open_documents`, **Then** I receive `[]` (not an error).

---

### User Story 3 - List Twin Folder Files With Pagination (Priority: P2)

As an AI agent
I want to enumerate every file in the currently-open Twin folder, classified by kind, with optional pagination
So that I can navigate the user's project without rewalking the filesystem myself

**Why this priority**: Enables Twin-aware automation (open the right model, find the right CSV). Pagination is opt-in because typical Twins have <100 files; needed only for outliers.

**Independent Test**: Issue `list_twin`, receive `{open: true, root: "<abs>", files: [...], total: N}` when a Twin is open, or `{open: false}` when none is.

**Acceptance Scenarios**:

1. **Given** a Twin is open, **When** I call `list_twin`, **Then** I receive every file under the Twin root (excluding `twin.toml` and `.lunco/`), each tagged with its `FileKind` classification.
2. **Given** a Twin with 500 files, **When** I call `list_twin` with `limit=100, offset=200`, **Then** I receive entries 200..299 in stable filename order.
3. **Given** no Twin is open, **When** I call `list_twin`, **Then** I receive `{open: false}` with no `files` array.

---

### User Story 4 - List MSL Classes With Filters and Pagination (Priority: P2)

As an AI agent
I want to enumerate Modelica Standard Library classes with prefix, category, and "examples-only" filters, paginated
So that I can find a class to open without reading 2500 entries at once

**Why this priority**: MSL is large enough that a single dump would blow agent context. Pagination + filters are required, not optional.

**Independent Test**: Issue `list_msl(filter:{examples_only:true, prefix:"Modelica.Blocks"}, limit:50)`, receive ≤50 example classes from `Modelica.Blocks.*` plus a `next_cursor` for the rest.

**Acceptance Scenarios**:

1. **Given** the MSL library is loaded, **When** I call `list_msl(limit:200)`, **Then** I receive 200 entries plus a `next_cursor` token I can pass back for the next page.
2. **Given** I pass `filter:{prefix:"Modelica.Electrical"}`, **When** I call `list_msl`, **Then** every entry returned has a qualified name starting with `Modelica.Electrical`.
3. **Given** I pass `filter:{examples_only:true}`, **When** I call `list_msl`, **Then** every entry returned has `.Examples.` in its qualified path.
4. **Given** I pass `filter:{category:"Blocks"}`, **When** I call `list_msl`, **Then** every entry has `Modelica.Blocks` as its top-level package.
5. **Given** the MSL library has not finished prewarming, **When** I call `list_msl`, **Then** the call still returns valid data — even at the cost of blocking — and `msl_status` reports `loaded:false` until prewarm completes.

---

### User Story 6 - Unified `Open` With Scheme Dispatch (Priority: P1)

As an AI agent
I want a single `open(uri)` command that figures out the source from the URI scheme
So that I do not need to know whether the target is bundled, MSL, on disk, or already open

**Why this priority**: Collapses the three existing open commands (`OpenFile`, `OpenClass`, `OpenExample`) plus the missing `bundled://` and `mem://` paths into one entry point. The existing primitives stay as fallbacks; nothing is removed.

**Independent Test**: Issue `open("bundled://RC_Circuit.mo")`, `open("Modelica.Blocks.Examples.PID_Controller")`, `open("/abs/path/foo.mo")`, `open("mem://Untitled3")`. Verify each lands the correct doc on screen.

**Acceptance Scenarios**:

1. **Given** I pass a `bundled://Filename.mo` URI, **When** I call `open`, **Then** the embedded model opens as a new Untitled tab (matching today's Welcome-tab behaviour).
2. **Given** I pass a qualified Modelica name with no scheme (dot-separated, no `/`), **When** I call `open`, **Then** the call is dispatched to `OpenExample` and the MSL class opens as a duplicated Untitled tab.
3. **Given** I pass an absolute filesystem path, **When** I call `open`, **Then** the call is dispatched to `OpenFile` and the file opens.
4. **Given** I pass a `mem://Name` URI for a doc that is already open, **When** I call `open`, **Then** the existing tab is focused (no new tab created).
5. **Given** I pass a malformed or unresolvable URI, **When** I call `open`, **Then** I receive an `ApiResponse::Error` with `CommandNotFound` or `EntityNotFound` and a human-readable message.

---

## Requirements

### Functional Requirements

- **FR-001**: `list_bundled` MUST return every `*.mo` file embedded under `assets/models/`, each entry carrying at minimum `filename` and `tagline`.
- **FR-002**: `list_open_documents` MUST return one entry per workspace document (saved + Untitled), carrying `doc_id`, `title`, `origin`, `kind`, `dirty`, `active`, `view_mode`. Untitled docs MUST be included alongside file-backed ones.
- **FR-003**: `list_twin` MUST return `{open, root, files, total}` when a Twin is open, or `{open:false}` when none is. Files MUST carry the `FileKind` classification produced by `Twin::index`. The endpoint MUST accept optional `limit` and `offset` parameters.
- **FR-004**: `list_msl` MUST return a paginated slice of MSL classes with an opaque `next_cursor` token, accepting at minimum `prefix`, `category`, `examples_only` filters and a `limit` parameter (default 200, max 1000). The first call may block briefly on `MSL_LIBRARY` initialization; subsequent calls hit the cached `OnceLock`. A separate prewarm-status endpoint is intentionally not provided — by the time any API request arrives, the startup-time prewarm thread has had ample wall time to finish, and a status check would only add round-trips and TOCTOU between status and use.
- **FR-005**: `open(uri)` MUST dispatch on URI scheme: `bundled://` → embedded source as Untitled, qualified name → MSL example, absolute path → file open, `mem://` → focus existing tab.
- **FR-006**: `OpenFile` MUST recognize `bundled://` URIs and open the embedded source as an Untitled doc, preserving the existing fs-path behaviour for absolute paths.
- **FR-007**: All listing endpoints MUST return structured data via `ApiResponse::Ok { data: ... }` — not as console-log side effects.
- **FR-008**: Endpoints MUST be reachable both via the existing `POST /api/commands` HTTP endpoint and as typed MCP tools in `mcp/src/index.js`.

### Key Entities

- **`ApiQueryProvider` (new trait, `lunco-api`)**: A registered query provider that answers a typed request for a list of items. Domain crates (`lunco-modelica`, `lunco-workspace`) register implementations for bundled, twin, MSL, and open-documents queries. Keeps lunco-api free of domain knowledge.
- **`ListBundledRequest` / `ListTwinRequest` / `ListMslRequest` / `ListOpenDocumentsRequest` / `MslStatusRequest` / `OpenRequest` (new `ApiRequest` variants)**: Transport-agnostic enum variants. The transport layer parses them from the same `command` field used today; the executor routes them to the registered query providers.
- **`PaginationCursor` (new, `lunco-api`)**: Opaque base64-encoded JSON `{offset: u32, filter_hash: u64}`. Filter hash invalidates cursors when filter parameters change between calls — caller must restart pagination if they swap the filter.
- **`MslLoadFlag` (new, `lunco-modelica`)**: `AtomicBool` set true at the end of `prewarm_msl_library`. Read by `msl_status` and consulted by `list_msl` to decide whether to log a "blocking on prewarm" warning.

---

## Success Criteria

- **SC-001**: An agent can answer "what can I open right now?" with a single round trip per source (4 calls maximum: `list_bundled`, `list_twin`, `list_msl`, `list_open_documents`).
- **SC-002**: An agent can open any of the four source kinds with a single `open(uri)` call, regardless of source.
- **SC-003**: `list_msl` with `limit:200` returns in <50 ms after prewarm completes; `msl_status` returns in <1 ms regardless of prewarm state.
- **SC-004**: `lunco-api` keeps zero direct dependencies on `lunco-modelica` — domain knowledge is plugged in via `ApiQueryProvider`.
- **SC-005**: Bundled examples are openable via API on `wasm32` builds (no absolute path reachable, only `bundled://`).
- **SC-006**: Existing `OpenFile`/`OpenClass`/`OpenExample` commands remain functional and unchanged in behaviour for callers that already use them.

---

## Out of Scope

- Telemetry / signal listing (not a "model source" — a separate concern).
- Writing or modifying the Twin folder via API (read-only listing only).
- Full-text search across MSL or bundled sources (filter-only; no text indexing in this spec).
- Watching the Twin for filesystem changes and pushing notifications (separate `subscribe_*` concern).
- Migrating the existing `OpenFile`/`OpenClass`/`OpenExample` callers to `Open(uri)` — they keep working.

## Assumptions

- The active `Twin` is reachable from `lunco-workspace::WorkspaceResource`. (Verified: `Twin::files()` exists at `crates/lunco-twin/src/lib.rs:288`.)
- `bundled_models()` enumerates `assets/models/` deterministically and is cheap to call (already used per-frame by the Welcome tab).
- The MSL prewarm thread (`prewarm_msl_library`, `commands.rs:2050`) currently has no completion signal — adding an `AtomicBool` is the minimum viable change.
- The `mem://Name` scheme used internally by the Welcome tab and Package Browser is already a stable id; promoting it to a public URI is a labelling change, not a new identifier system.
- Cursor-based pagination is acceptable for MSL despite being stateless (the cursor encodes the offset and a filter hash).

## Implementation Phases

For tracking and review, implementation lands in six PR-sized phases:

- **P1** — `ApiQueryProvider` trait + plugin in `lunco-api` (no behaviour change).
- **P2** — `list_bundled` + `list_open_documents` providers (smallest surface; validates the pattern end-to-end).
- **P3** — `list_twin` provider with pagination.
- **P4** — `list_msl` provider + `msl_status` + `MslLoadFlag` AtomicBool.
- **P5** — `OpenFile` `bundled://` scheme support + unified `Open(uri)` dispatcher.
- **P6** — MCP typed tool wrappers in `mcp/src/index.js` + docs updates (`mcp/README.md`, `docs/api.md`, this spec's status → Implemented).
