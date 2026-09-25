> Status: Active · Audience: engine, scripting, and Twin authors

# Hook policies

A hook is a function-shaped decision seam. The Rust owner defines the smallest
fact boundary and a typed callable contract; an authored Rhai policy supplies
the implementation. Rust does not name a policy file, lesson, model, or
application choice.

## One declaration at the owner

The owner places `declare_hook!` beside the seam it exposes:

```rust
lunco_hooks::declare_hook! {
    id: CAMERA_PRESENTATION_HOOK,
    owner: "lunco-camera-core",
    description: "Choose the initial camera presentation action.",
    signature: [ctx: Map],
    output: String,
    deterministic: false,
    required: false,
    installable: true,
}
```

The macro submits one link-collected declaration. There is no application
registry or manually maintained list. The signature records named positional
parameters and closed `HookValueType` values (`Map`, `Bool`, `Int`, `Float`,
`String`, arrays, and explicit unions). The owner remains responsible for the
meaning of fields inside a `Map`; those facts are documented with the seam and
are not duplicated as a second Rust schema.

The hook substrate validates argument count and boundary types before invoking
the implementation, then validates its returned type. A missing implementation
is distinct from a fault in an installed function. The owner decides whether
an absent optional policy is valid; a required seam reports a structured fault.
No invalid result is converted into a guessed value.

## When behavior should be a hook

Hook candidacy is part of every design review. Use a hook when behavior is
expected to vary by application, Twin, authored scene, deployment, lifecycle
state, presentation mode, routing rule, or user policy. The owner publishes
facts and a typed result; Rhai selects the decision. This is not limited to a
single scalar choice: a hook can return a nested `Map`, an ordered `Array`, or
an `Array<Map>` describing a validated decision or action plan. The owner then
consumes that result through its generic engine mechanism. A hook whose output
has no consumer is an invalid design; an event-only notification must declare
`Unit` instead.

Keep continuous equations and state in Modelica, fast kinematics/dynamics and
invariants in Rust, and USD identity/topology in USD. Rhai owns the changeable
policy and scenario glue around those mechanisms. A new Rust branch is
justified only when it is the authoritative generic mechanism, a hot path, a
validation boundary, or a calculation that cannot be expressed safely by the
existing Rhai-facing surface. Each hook must document its owner, typed inputs
and output, installation scope, lifecycle, failure/required semantics, and a
production Rhai test.

## Runtime policy selection

Application policies are selected at simulation startup from the unique
authored TOML manifest marked `kind = "lunco.policy.v1"` in the runtime asset
tree. The repository ships that manifest at
`assets/scripting/policy/index.toml`, but its location is not a Rust path
contract. The manifest has one `[startup]` entry. That entry names the Rhai
function that receives the manifest-resolved policy records and installs them
through the private typed bootstrap binding. Each
`[[policies]]` entry names a hook id, a relative Rhai source file, an entry
function, and its determinism/required declaration. A policy owned by an
optional runtime feature may set `skip_when_hook_unavailable = true`; the
selected composition then leaves it out when its hook owner is not linked and
reports the hook in `policy_status().unavailable`. This flag skips only an
absent owner. Missing source, Rhai compile errors, and invocation failures
remain visible; a required policy cannot be skipped. A Twin may provide an
independent policy manifest under its own root; its records are merged
with application records (Twin entries replace the same hook id) and its own
startup function receives the Twin records that it owns when that Twin becomes
active. A Twin manifest with policy records must declare its own `[startup]`;
an empty Twin policy directory may omit it. Source bytes are resolved by the
asset/storage layer.

The Rhai policy bootstrap runs in `PreStartup`, before startup systems consume
authored policies. For example, rendering quality is owned by the
`lunco-render` seam: `render.quality_profile(id: String) -> Map` supplies the
complete typed settings for one stable profile id, while
`render.default_quality_profile() -> String` chooses the initial id for fresh
settings. The application manifest installs both from
`assets/scripting/policy/render_quality_profiles.rhai`; Rust validates map
shape, types, bounds, and shadow allocation before installing the catalog in
`Startup`. An active Twin can replace either hook through its policy manifest,
and a changed committed policy revision causes the catalog to be resolved
again. Replacing several hooks in one policy transaction therefore produces
one catalog/source admission pass, rather than one pass per low-level hook
registration.

Both rendering hooks are deterministic, required, and installable. The render
owner keeps the stable ids and typed field contract; Rhai owns all profile
values and the fresh-settings selection. A missing hook, invalid id, malformed
map, or rejected profile leaves the catalog unavailable with a visible
diagnostic. There is no Rust preset table or fallback profile. Existing valid
persisted custom settings remain authoritative if the catalog cannot load.
The production Rhai test verifies declarations, the High default, representative
values for each profile, and failure for an unknown id in
`assets/scripting/tests/test_hook_policies.rhai`.

The application startup policy is run once when the simulation starts. It may
install a generic lifecycle hook such as `twin.lifecycle`. The active Twin
invokes that hook for `startup`, `reload`, and `close`; the application policy
is therefore the broad default, while a Twin startup policy can replace the
hook for its own authored behavior. A missing optional lifecycle policy is a
valid unconfigured state, and a lifecycle fault is reported without crashing
or silently selecting another implementation.

Every `twin.lifecycle` invocation carries `Twin/Lifecycle` with the mounted
Twin's nonzero `TwinId` as its generation, `RuntimeClock::None`, and the phase
`Start`, `Event`, or `Stop`. `TwinId` is allocated monotonically for each mount
within the owning workspace, so a later mount of the same path has a distinct
route. `policy_status().lifecycle.runtime_context` retains the exact context
used for the latest delivery.

The generic asset layer owns asynchronous JSON reads and publishes
`JsonAssetScopeLoading` and `JsonAssetScopeChanged` events for the engine
library and each opened Twin. The application manifest installs the optional,
installable `application.asset.lifecycle(event: String, ctx: Map) -> Map` hook
before those events can fire; the shipped application manifest requires this
policy to install successfully. Its context carries the stable `provider`, the
`scope`, optional `twin_id` and `twin_name`, and a `payload`: loading events
provide `has_json_assets`; changed events provide the complete array of
`{asset_uri, text, error}` records for that scope. Both contexts include the
asset owner's canonical `asset_root_uri` for scope-relative references.
`parse_json(text)` is shared by the world-bound Rhai engine and Rhai policy
hooks. Rhai returns `#{ menus: [...], dataset_text_artifacts: [...] }`. The
optional `dataset_text_artifacts` array contains declared dataset ids to read.
The scene completion context provides the loaded `path` and `root_prim`; the
Rhai policy chooses which artifacts that scene uses. Rust validates the
complete action map, then the generic asset runtime resolves registry ids to
canonical `lunco://` or `twin://` URIs and loads text through the shared
`TextAsset` loader. Missing or unreadable requested datasets are reported;
unrequested datasets remain quiet. `SceneTransitionStarted` retires outstanding reads and
`SceneTeardown` removes domain data derived from the outgoing scene.

Rust also validates the generic menu tree and replaces that provider's
contribution; selected actions route through the existing Rhai tool hook. Twin
contributions are cleared on `TwinClosed`, and a late contribution for an
already-closed Twin is discarded. An absent optional hook clears its
contribution; a fault or malformed action map is warned and queues no asset
reads. This is the startup-to-menu path used by the authored tutorial catalog
policy, so no Rust tutorial menu or catalog parser is needed.

Physics owns the optional deterministic
`physics.initialization(facts: Map) -> String` seam for explicit pre-admission
decisions and the required deterministic `physics.body_escape(ctx: Map) ->
String` seam, installed by the application policy bootstrap during `PreStartup`
and replaceable by an active Twin. The escape map contains `kind` (`finite_world_exit`
or `non_finite_state`), `path` and `global_id` (each a string or `Unit` when
unavailable), `position_m`, `velocity_mps`, `world_min_m`, and `world_max_m`
(three-float arrays for a bounded world, otherwise `Unit`). The application
Rhai policy returns `pause_object` for either condition; Rust applies it to the
dynamic joint-connected island, its joints, and its colliders so the stopped
object cannot affect other bodies. Physics and world time continue for
everything else. An authored policy can return `pause_world` when a Twin
intentionally needs that response. Missing, faulting, or malformed policies
fail closed with a visible runtime fault and physics hold.

The initialization selector is authored through
`LunCoPhysicsInitializationAPI`; Rust dispatches every selected body through
the one declared seam and supplies a `Twin/Lifecycle/Preparation` context
with no elapsed clock. Facts contain the stable USD subject path, selector,
finite pose, and assembly member count, never ECS entity ids. Missing schema or
selector, a missing or non-deterministic policy, wrong context, or a malformed
or rejected result leaves the body held and publishes a runtime diagnostic.
The built-in `strict-authored` path does not invoke Rhai.
`assets/scripting/tests/test_hook_policies.rhai` covers the required contract
and rejects an off-cycle invocation. The physics owner stamps each real call
with its core simulation route, `Behavior` phase, fixed clock sample, and
`SimTick`; `escape_containment` proves the installed policy accepts that
context, contains the escaped object, and keeps the real solver moving an
unaffected control body.

The lifecycle dispatcher validates the returned map, retains the typed result
as the current lifecycle record, and exposes it through `policy_status()` and
the API status view. On `assets_mounted`, the application policy receives
`twin_id`, the exact asset authority `name`, `root`, `active`, the parsed
`manifest` as a typed map (or `Unit` for a plain folder), and indexed relative
`files`. It returns an ordered `actions` array whose entries contain a reflected
`command` name and typed `params` map. The generic executor validates the plan
shape and dispatches each entry through the ordinary typed command bridge; it
has no Twin-field or domain-loader branches. The queue is applied only while
that Twin remains active. A malformed plan faults the lifecycle result and
queues no actions; an individual command rejection is logged and later actions
continue. This lets Rhai own the choice and ordering while domain owners retain
path validation and asynchronous loading mechanics. The application policy
selects the default USD scene, Twin tool libraries, timelines, SysML/KerML
source files, and Modelica roots from the typed manifest and indexed inventory.
If a lifecycle seam is only a notification, its contract should instead return
`Unit`.

For example, a Twin can keep its lifecycle policy beside its authored Twin:

```toml
# <twin-root>/<policy-manifest>.toml
[startup]
source = "startup.rhai"
entry = "install_twin_policies"

[[policies]]
hook = "twin.lifecycle"
source = "lifecycle.rhai"
entry = "twin_lifecycle"
```

That startup receives the one Twin-owned record and can install it through the
same typed bootstrap binding. It does not need to repeat application defaults
that it does not replace.

The startup function is the only bootstrap convention in each scope. It is
authored behavior and can choose installation order/reporting, while Rust
retains the generic typed installer, owner-contract validation, and cleanup
boundary. There is no second hardcoded list of policy files or source roles in
the engine. The required `scripting.source.classify` policy classifies
engine-library sources as preludes or unrelated scenario content; admission,
asset publication, and runtime preparation run in that order. During a policy
replacement, a temporarily unavailable classifier leaves the current admitted
sources intact and closes script execution until the authored policy returns.
Twin tool libraries are selected and loaded by the `twin.lifecycle` action
plan.

### Tool layers and shutdown

Tools use the same lifecycle distinction as policies. The runtime tool registry
keeps four independent layers: `standard` for shared `lunco://` libraries,
`core` for always-on native substrate tools, `application` for process-owned
dynamic registrations, and `twin:<id>` for authored Twin libraries. Resolution
is broad-to-narrow, with the active Twin winning over application, core, and
standard definitions of the same short library name. Discovery reports the
winning owner instead of flattening the source provenance.

The standard and Twin layers are selected by policy and are therefore
revocable. When `scripting.source.classify` stops admitting a standard tool,
the source owner retires that standard registration; when `TwinClosed` fires,
the Twin layer is removed and the lower layers become visible again. No
snapshot restore is used, so a Twin shutdown cannot resurrect an older tool
over a newer application registration. A rebuilt Rhai engine observes the
registry generation and removes retired static modules from subsequent script
execution.

The same process supports a large behavior surface: each subsystem declares
its own seam beside its owner, and the link-collected catalog exposes all
declared signatures to Rhai and API clients. A policy can compose existing
commands, queries, events, and tool libraries, or return a structured plan for
the owner’s generic executor. It must not reimplement USD composition, physics,
continuous math, or a second command bus inside Rhai. Product-specific choices
remain authored in policy files, while Rust keeps the small fact/validation/
execution boundary reusable across many policies.

When a composed USD scene authors `LunCoPolicy` prims, those policies form a
scene-owned layer above the application and Twin manifest layers. Removing a
USD policy restores the lower compiled layer; a malformed authored override
remains unavailable and blocks that seam rather than silently falling through.

Policy loading is dynamic. A source or manifest error is recorded in
`PolicyLoadReport` and exposed through `policy_status()`. A failed replacement
is removed rather than leaving an older implementation active. Optional hooks
therefore leave the owner in its documented unconfigured state; only a
manifest-declared `required = true` failure blocks that seam. Deterministic
hooks can be installed from an authored manifest only when the manifest opts
into `deterministic = true`; an inline binding cannot claim convergence.

## Runtime execution context

Scheduled owners pass a typed `RuntimeExecutionContext` with each hook call.
The immutable Rhai `runtime_context` map describes the owner's route, cycle,
phase, clock sample, sequence, and event producer where applicable. A hook
whose policy is valid in one cycle should reject other contexts. For example,
`readiness.action` runs as `Core/Simulation/Behavior` with fixed-clock time and
the latest `SimTick`; its policy rejects lifecycle, UI, and REPL calls. Test
both the production owner path and an intentional off-cycle call in authored
Rhai.

The `synth.acausal-network` and `synth.actuator-wrench` source generators run
as `Twin/Lifecycle/Preparation` with the active-or-committed scene generation
and no clock sample. The domain owner captures that context before async
dispatch and supplies the same value to synchronous live projection. Their
policies reject calls from other cycles; production tests exercise an off-cycle
call and verify source publication through the generated-source query. Async
results also carry the same Twin generation and are discarded when it changes,
even if their USD stage revision still matches.

## Native providers

An approved Twin may implement an existing installable hook with a native
shared library. The provider is listed explicitly in `twin.toml` under
`[[native_plugins]]`; it is never discovered from USD, a Rhai source file, or a
directory scan. The loader validates the provider descriptor against the
link-collected hook catalog and registers the callback through the same
`HookInvocation` path used by Rust and Rhai, including owner-supplied runtime
context. See
[`native-hook-providers.md`](native-hook-providers.md) for the ABI, lifecycle,
failure, and future terrain-kernel boundary.

Native code is trusted process code, not a sandbox. A plugin may provide an
expensive generic computation, but it must return typed data or an action plan;
the Rust owner validates and applies that result. It must not mutate ECS or USD
behind the owner’s boundary, and a domain-specific plugin must not become a
second dispatch registry. A future Chrono terrain provider belongs at the
consumed `lunco-terrain-bake` kernel contract, not in the terrain runtime
projection.

## Rhai and API surfaces

Rhai uses native values at the hook boundary:

```rhai
let hooks = list_hooks();
let result = invoke_hook("camera.default_presentation", [#{
    local_avatar_camera_count: 1,
    camera_track_count: 0,
    host_requests_generated: false,
}]);
```

`list_hooks()` reports `parameters: [{name, type}]`, `output`, owner,
determinism, requirement, policy binding, and installed backend. With
native-provider support enabled, `policy_status().native_plugins` also reports
loaded provider ids and admission failures. The API
`DiscoverSchema.hooks` exposes the same catalog with
`parameters: [{name, type_name}]`. `bind_policy`, `unbind_policy`, and
`invoke_hook` return structured operation results so denied, rejected,
unavailable, and faulted states remain visible. `policy_status()` also reports
the last Twin lifecycle event, status, typed result, runtime context, and any
delivery error.
Its `unavailable` list identifies feature-gated policies whose hook owner is
absent from the selected runtime composition; `failed` remains reserved for
policies that were selected but could not compile or activate.
JSON is only the outer API transport representation; internal hook calls use
`HookValue`, not JSON maps.

The scripting authoring catalog and `ScriptComplete` consume this same
link-collected catalog. They must not introduce a second hook list or infer a
signature from policy source text.

## Testing boundary

The production Rhai scene-test surface verifies catalog reflection, dynamic
binding, invocation, missing-policy diagnostics, deterministic-binding
rejection, and exact unbinding in
`assets/scripting/tests/test_hook_policies.rhai`. Rust tests remain limited to
the low-level owned-value conversion and registry mechanism; they do not repeat
observable policy behavior or load authored scene fixtures.
