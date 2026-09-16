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

Application policies are selected at simulation startup from
`assets/scripting/policy/index.toml`. The manifest has one `[startup]` entry.
That entry names the Rhai function that receives the manifest-resolved policy
records and installs them through the private typed bootstrap binding. Each
`[[policies]]` entry names a hook id, a relative Rhai source file, an entry
function, and its determinism/required declaration. A Twin may provide an
independent `policies/index.toml` under its own root; its records are merged
with application records (Twin entries replace the same hook id) and its own
startup function receives the Twin records that it owns when that Twin becomes
active. A Twin manifest with policy records must declare its own `[startup]`;
an empty Twin policy directory may omit it. Source bytes are resolved by the
asset/storage layer.

The application startup policy is run once when the simulation starts. It may
install a generic lifecycle hook such as `twin.lifecycle`. The active Twin
invokes that hook for `startup`, `reload`, and `close`; the application policy
is therefore the broad default, while a Twin startup policy can replace the
hook for its own authored behavior. A missing optional lifecycle policy is a
valid unconfigured state, and a lifecycle fault is reported without crashing
or silently selecting another implementation.

The lifecycle dispatcher validates the returned map, retains the typed result
as the current lifecycle record, and exposes it through `policy_status()` and
the API status view. This makes a structured lifecycle decision observable to
the owner and to diagnostics; it is not an ignored side effect. If a lifecycle
seam is only a notification, its contract should instead return `Unit`.

For example, a Twin can keep its lifecycle policy beside its authored Twin:

```toml
# <twin-root>/policies/index.toml
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
boundary. There is no second hardcoded list of policy files in the engine.

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

## Native providers

An approved Twin may implement an existing installable hook with a native
shared library. The provider is listed explicitly in `twin.toml` under
`[[native_plugins]]`; it is never discovered from USD, a Rhai source file, or a
directory scan. The loader validates the provider descriptor against the
link-collected hook catalog and registers the callback through the same
`lunco_hooks::invoke` path used by Rust and Rhai. See
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
the last Twin lifecycle event, status, typed result, and any delivery error.
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
