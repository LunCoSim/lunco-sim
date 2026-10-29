# Twin navigation

The workbench must identify the active Twin, the loaded world, and the source
being edited independently. A source preview is a document operation; scene
replacement remains an explicit load action.

The persistent context bar names the Twin and scene across perspectives. Editor
instances publish exact source URI/purpose metadata. Twin-owned source paths
become clickable breadcrumbs; source reveal filters the existing Navigator.
Navigator starts with active-Twin files and collapsed secondary sections.
Libraries and document dependencies are explicit lenses/groups. Editor places
Navigator first beside Prims through the existing Rhai perspective policy.

## Ownership and reuse

- `lunco-workbench-core` owns navigation provider/source contracts and typed
  navigation commands. Providers publish readable labels, canonical identities,
  and callbacks that emit existing typed domain intents.
- `lunco-workbench` owns the context/search presentation and bounded transient
  document/perspective history. Activation resolves the exact identity against
  the current provider inventory; closed or unavailable destinations reject.
- `lunco-workbench-browser` owns lens/filter state, the cached active-Twin file
  tree, file dispatch and reveal. Source editing, rename and USD opening retain
  their document owners. Library rows use canonical source identity for the
  loaded marker.
- `lunco-assets-runtime::discovery::list_indexed_scene_assets` reads admitted
  Twin manifests/inventories and the library manifest. Scenarios and search
  consume it without filesystem traversal or manifest parsing during paint.
- USD, Modelica and generic source editors publish their own source context;
  the shell never interprets an instance number as document identity.

The provider registry is a discovery seam, not another file index or dispatcher.
The file-tree projection rebuilds only on workspace/filter changes. Search calls
only relevant providers for an explicit scope and virtualizes the result list.
Missing/loading providers expose their reason alongside any usable results.

## Policy and lifecycle review

The existing `workbench.perspectives.defaults` Rhai hook remains the owner of
application task layout, including Navigator placement. Provider and section
metadata declare available destinations and section defaults. Keyboard handling,
exact URI routing, search matching, history application and stale-owner rejection
are generic UI mechanisms. There is no Twin-name-specific routing or additional
mission/lifecycle policy requiring a second Rhai hook.

Search, history and browser filters are transient. Active `TwinClosed` clears
them and resets the Twin lens; inactive closure preserves the current lens.
Queued navigation pins its admitted Twin identity. File trees clear immediately
on active closure and republish from the incoming inventory. Document ownership
and editor lifetime remain with their existing registries.

## Verification

Build the production app with `cargo build -p lunco-luncosim --bin luncosim -j 4`.
Run `LUNCOSIM_BIN=target/debug/luncosim python3 scripts/api/test_workbench_navigation.py --port <free-port>`.
Its authored Rhai gate observes exact context, source previews, library lookup,
history, explicit scene choices, empty/rejected destinations, native application
shortcut events, and Twin retirement. Inspect the captured preview/search/action
screens and the default Navigator in a populated owned session. Injected native
events exercise the application event path, not physical OS device delivery.
The existing workbench perspective gate covers the Rhai layout consumer.

## Command bindings and status space

The shared input settings own semantic controls and reflected command shortcuts.
The workbench settings panel writes through validated typed setters and the
existing settings persistence. Twin overrides remove physical collisions from the
effective global map and publish winner/displaced warnings in Recent Events.
Global preferences are preserved, and Twin closure restores them. The document
owner exposes `InvokeEditorIntent`; all document shortcut dispatch uses this
adapter and the existing domain resolvers. No independent editor keymap remains.

The reflected command core is reused without enabling a network transport.
Scalar arguments cross the internal boundary as typed `HookValue`; JSON remains
at the authored-document/persistence boundary. Commands requiring structured
arguments can be composed behind a typed parameterless command rather than
adding an untyped shortcut payload. Focus rules apply to both command dispatch
and semantic key claims. Pending shortcut outcomes retire with their scene.
The generic core owns these mechanisms; shortcut definitions remain authored
data, and Rhai owns the settings perspective.

The latest-message slot reserves 20% more preferred/minimum width and raises its
status-strip width share by 20%, retaining the existing full-message hover and
Recent Events access.

Both new authored acceptance scenarios use behavior trees. Run
`scripts/api/test_command_keybindings.py --port <free-port>` for native command
dispatch, invalid-command rejection, W/K override collisions, Recent Events
notifications, non-repeating notices and restoration after Twin replacement.

The input-profile fixture remains under `assets/scenes/tests`; its `.json.in`
source is staged as `input.json` in the driver's output Twin. This keeps the
fixture out of the bundled application-default JSON scope while using normal
Twin admission. Runtime acceptance passes 18 navigation checks, 11 binding
checks and the existing 23 perspective checks; screenshots cover collapsed
Navigator defaults, source context, search scopes and the scrollable settings
form.
