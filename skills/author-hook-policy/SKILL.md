---
name: author-hook-policy
description: Declare, bind, inspect, and test a LunCoSim function-shaped Rhai hook policy through the owner macro and authored policy manifest.
---

# Author a hook policy

Use this skill when a decision should be changeable without recompiling the
engine. A hook is a function-shaped seam: Rust publishes a small typed fact
boundary and Rhai supplies the policy implementation.

Treat hook candidacy as a mandatory design step. Prefer a hook for changeable
lifecycle, routing/selection, presentation, permission, deployment, or
Twin-specific behavior. A hook may expose substantial behavior with nested
maps/arrays and return a typed decision or action plan. Its owner must validate
and consume that result through a generic mechanism; use `Unit` for a pure
notification and never leave a decision result unread. Keep continuous math,
kinematics, dynamics, invariants, and hot paths in Rust or Modelica. Before
adding a Rust branch, identify the owner, fact inputs, result consumer,
installation scope, lifecycle, failure/required semantics, and the production
Rhai test.

## Read first

- [`AGENTS.md`](../../AGENTS.md) for ownership, failure, asset, and test rules.
- [`hook-policies.md`](../../docs/architecture/hook-policies.md) for the
  active hook contract.
- [`native-hook-providers.md`](../../docs/architecture/native-hook-providers.md)
  when a trusted native implementation or an expensive domain kernel is under
  consideration.
- [`rust-rhai-modelica-boundary`](../rust-rhai-modelica-boundary/SKILL.md) when
  deciding whether the seam belongs in Rust, Rhai, USD, or Modelica.
- [`validate-assets`](../validate-assets/SKILL.md) for authored production
  scene-test execution.

## Declare the seam at its owner

Place one `lunco_hooks::declare_hook!` invocation beside the owner’s hook id
and decision function. It is collected automatically; do not edit a central
list. Declare the function-shaped ABI with named parameters and a
`HookValueType` for each parameter and the result:

```rust
lunco_hooks::declare_hook! {
    id: MY_HOOK,
    owner: "my-owner",
    description: "Choose a generic engine action from authored facts.",
    signature: [ctx: Map],
    output: String,
    deterministic: false,
    required: false,
    installable: true,
}
```

Keep the map contents as owner facts, not a second ad-hoc registry. Use a
deterministic contract only when identical inputs must produce the same result
on every peer. Set `required` only when the generic mechanism cannot operate
safely without a policy.

For scheduled or async owner work, consume the `runtime_context` supplied by
the hook owner and reject calls from the wrong cycle or phase. The owner must
capture the context before dispatch, preserve it through worker completion,
and reject stale results before publication. Test the scheduled path and an
intentional off-cycle call in production authored Rhai.

## Author and select the policy

Put the implementation in `assets/scripting/policy/<name>.rhai` and add one
entry to `assets/scripting/policy/index.toml`:

```toml
[[policies]]
hook = "my.hook"
source = "my_policy.rhai"
entry = "decide"
deterministic = false
required = false
```

The manifest's single `[startup]` entry names the Rhai function that receives
and installs all resolved policy records. The application manifest is loaded
at simulation startup. A Twin may provide its own uniquely marked policy manifest; its
matching entries replace application records before its own startup function
runs when that Twin becomes active. The Twin startup function receives the
Twin-owned records; application policies remain active for seams the Twin does
not replace. A Twin manifest that contains policy records must declare its own
`[startup]` source and entry; an empty Twin policy directory may omit it. Do
not add a Rust-side policy list or a second bootstrap path.
When `skip_when_hook_unavailable = true`, the runtime omits the policy only
when that hook owner is not linked into the selected build and reports its id
in `policy_status().unavailable`. Source, compile, and activation failures
remain visible, and required policies cannot use this flag.
The source path is resolved by the asset/storage layer. Do not read policy
files through `std::fs` in a runtime/domain crate.

Inline `bind_policy(id, entry, source)` is useful for a local non-deterministic
experiment. It must not claim a deterministic contract. Use `unbind_policy`
to remove exactly that implementation; reloading the authored manifest is the
explicit operation that installs the authored policy again.

For a trusted native implementation, add an explicit `[[native_plugins]]`
entry to the Twin manifest. The provider must implement an existing reflected
installable hook and return the declared typed result through the native ABI;
it does not declare a second hook list or mutate the owner's ECS/USD state.
Use a native provider for expensive or platform-specific computation, not for
changeable product policy. An eventual terrain provider belongs at the
consumed `lunco-terrain-bake` boundary.

## Inspect and test

Use `list_hooks()` to inspect the reflected declaration. It reports
`parameters: [{name, type}]`, `output`, ownership, policy binding, determinism,
requirement, and installation state. Use `policy_status()` for startup/Twin
load diagnostics and the last typed Twin lifecycle result. The owner must
consume every non-`Unit` result; the lifecycle dispatcher retains its returned
map and exact invocation context in that status surface. Twin lifecycle calls
use a `Twin/Lifecycle` route whose generation is the mounted Twin's nonzero
`TwinId`; startup/reload, asset events, and teardown use `Start`, `Event`, and
`Stop` phases with no elapsed clock. The `assets_mounted` lifecycle event receives the
parsed Twin manifest, indexed relative paths, exact `twin://` authority, and
active-Twin fact. Its ordered `{command, params}` actions go through the generic
typed command bridge; Rhai chooses loaders and order, while each domain owner
validates paths and performs asynchronous work. A malformed plan faults and
queues no commands; a rejected command is reported while later actions
continue. `invoke_hook(id, [args])` distinguishes unavailable hooks from
installed functions that fault.

Scheduled owner calls supply an immutable `runtime_context` map with their
route, cycle, phase, clock, and logical sequence. If a policy is valid only in
one cycle, reject other contexts and verify the real scheduled owner path in
an authored production test. For example, `readiness.action` runs under
`Core/Simulation/Behavior` with fixed-clock time and the latest `SimTick`; its
policy rejects direct calls from other cycles.
Physics initialization is a discrete lifecycle decision: use the declared
`physics.initialization(facts)` seam, a generation-qualified
`Twin/Lifecycle/Preparation` route, and no clock sample. Keep authored
selector names in `facts.policy`; do not construct dynamic hook ids or expose
process-local ECS entity ids as deterministic policy facts.

The application `twin.lifecycle` policy selects the active Twin's default USD
scene, tool libraries, timeline data, SysML/KerML sources, and Modelica roots
from its typed manifest and indexed file inventory. Loader commands keep only
generic ownership, safe-path, asset-read, and domain-registration mechanics.

For application UI contributions, reuse the optional
`application.asset.lifecycle(event, ctx)` hook. The shared asset layer emits
JSON-scope loading/changed events after asynchronous reads, including the
canonical `asset_root_uri`; the Rhai policy can parse each record with
`parse_json(text)` and return a generic `menus` tree. Scene completion is also
delivered with the canonical loaded `path` and `root_prim`; return selected declared
dataset ids in `dataset_text_artifacts` when the scene needs their text. The
Rust host validates the complete action map, while `DatasetArtifactPlugin`
resolves each registry id to its canonical asset URI and reads through the
shared `TextAsset` loader. Missing or unreadable requested datasets are
reported; unrequested datasets remain quiet. `SceneTransitionStarted` retires pending reads. Keep
asset selection and menu policy in Rhai; Rust owns typed lifecycle facts, safe
asset delivery, action-shape validation, rendering, and provider cleanup.

Put policy and observable runtime assertions in an authored Rhai production
scene test. Cover the declared signature, successful binding/invocation,
missing-policy status, rejected deterministic inline binding, and exact
unbinding. Keep Rust coverage to the low-level `HookValue` conversion and
registry mechanics; do not embed long USD or load Twin assets in Rust tests.

## Handoff

Report the owner-side declaration, manifest/source files, reflected signature,
failure semantics, focused checks, and the production Rhai test evidence.
