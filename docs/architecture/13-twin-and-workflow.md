# 13 — Twin and Workflow

> **Twin** is LunCoSim's top-level persistent artifact: a folder of related
> domain-standard files plus a tool-specific manifest. This doc defines
> what's in a Twin, how it loads/saves, how users work inside and outside
> a Twin, and how cross-document references survive edits.

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

## 4. Orphan documents — working outside a Twin

Users can open any Document without creating a Twin:

```sh
$ modelica_workbench balloon.mo
$ rover_sandbox_usd scene.usda
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

## 6. Startup flow

All three apps (`rover_sandbox_usd`, `lunco_client`, `modelica_workbench`)
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
| `modelica_workbench` | Modelica-only models |
| `rover_sandbox_usd` | 3D sandbox scenarios |
| `lunco_client` | All examples |

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

The three binaries — `modelica_workbench`, `rover_sandbox_usd`,
`lunco_client` — share the **same Twin-loading machinery** from
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
| `modelica_workbench` | **Analyze** | `ModelicaPlugin` + `ModelicaInspectorPlugin` | Modelica examples only (circuit, spring-mass, thermal, …) |
| `rover_sandbox_usd` | **Build** | `CoSimPlugin`, `ModelicaCorePlugin`, `SandboxEditPlugin`, `EnvironmentPlugin`, `UsdPlugins`, `Mobility`, `Controller`, `Avatar`, … | Sandbox examples (rover-on-moon, balloon-test, …) |
| `lunco_client` | **Build** (or last-used) | All of the above + `CelestialPlugin` + `LuncoUiPlugin` (MissionControl) | All examples, categorized |

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
// modelica_workbench (before)
fn main() {
    app.add_plugins(DefaultPlugins)
       .add_plugins(EguiPlugin::default())
       .add_plugins(bevy_workbench::WorkbenchPlugin { ... })
       .add_plugins(ModelicaPlugin)
       .add_systems(Startup, setup_sandbox);   // hardcodes Battery.mo
}

// modelica_workbench (after)
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
| `modelica_workbench` | New Modelica Model, New Modelica Package |
| `rover_sandbox_usd` | New Scene (USD), New Twin, New Modelica Model |
| `lunco_client` | New Scene, New Modelica Model, New Mission, New SysML Block, New Twin |

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

See [30-collab-roadmap.md](30-collab-roadmap.md) (TBD) for design.

## 14. See also

- [`00-overview.md`](00-overview.md) — three-tier architecture
- [`01-ontology.md`](01-ontology.md) — Document, DocumentOp, DocumentView definitions
- [`10-document-system.md`](10-document-system.md) — the editable-artifact pattern that lives inside a Twin
- [`11-workbench.md`](11-workbench.md) — the UI that hosts Twin editing
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica Document specifics
- [`21-domain-usd.md`](21-domain-usd.md) — USD Document specifics
