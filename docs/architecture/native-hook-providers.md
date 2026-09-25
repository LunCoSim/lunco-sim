> Status: Active · Audience: engine, Twin, Rhai, and native-provider authors

# Native hook providers

LunCoSim has one hook catalog and one invocation path. A hook owner declares a
typed, reflected contract with `lunco_hooks::declare_hook!`; the implementation
may be authored in Rhai or supplied by a native provider. Native providers are
an opt-in acceleration and integration boundary for work that cannot run
efficiently in Rhai, such as a domain-specific terrain kernel.

## Provider forms

There are two implementation forms for the same contract:

| Provider | Installation | Appropriate work |
|---|---|---|
| Rhai policy | Application or Twin uniquely marked policy manifest | Changeable policy, routing, lifecycle, scenario glue, and authored behavior |
| Native provider | Twin `[[native_plugins]]` manifest entry | Trusted, expensive, or platform-specific computation behind an existing hook |

Rhai policies do not create a second command or hook registry. Native providers
also do not create a parallel registry: they register only an existing
`installable` hook id. Owners invoke it through `lunco_hooks::invoke` with a
`HookInvocation` containing validated positional values and an explicit typed
runtime context. Discrete boundaries use the explicit unclassified context;
cycle-owned calls supply their route, clock sample, and phase.

The native substrate is split into two small packages. `lunco-hooks-plugin-api`
contains only the edition-2024 C ABI descriptor and the shared typed wire
helpers. `lunco-hooks-native` owns the operating-system library loader, unsafe
pointer boundary, admission checks, callback buffer negotiation, and teardown.
The dependency-light `lunco-hooks` registry remains the owner of hook contracts
and does not need to compile the dynamic-loader path.

## Twin approval and lifecycle

A Twin explicitly approves providers in `twin.toml`:

```toml
[[native_plugins]]
id = "chrono-terrain"
path = "plugins/libchrono_terrain.so"
enabled = true
```

The path is Twin-relative, must not be absolute or contain `..`, and is
resolved without probing the filesystem during manifest parsing. USD and Rhai
source cannot cause a library to load. Native libraries are trusted native
code; the loader is an admission and lifecycle boundary, not a security
sandbox. A deployment that accepts untrusted Twin bundles must add signature,
provenance, and OS-level isolation before enabling this feature.

When the active Twin starts or reloads, the application composition loads its
enabled entries before Twin policy startup. When the Twin closes or is
replaced, its lifecycle policy is invoked first, then the provider registrations
are removed, and only then is the Twin policy state wound down. A provider
registration retains its library until it is removed. Teardown compares the
exact registration identity, so an older provider cannot remove a newer
implementation that took over the same hook id.

Each load is all-or-nothing. The host rejects malformed descriptors, unsupported
ABI versions, unknown or non-installable hook ids, determinism mismatches,
duplicate capabilities, missing callbacks, and occupied hook ids. A rejected
provider leaves no partial registration. Optional provider failures remain in
the Twin-scoped diagnostic report; they do not crash the application or turn
into a fabricated result. The owning hook still decides whether an unavailable
implementation is valid, required, or a runtime fault.

## Typed native boundary

The ABI v2 callback receives one bounded versioned binary invocation containing
the positional arguments and the owner-supplied runtime-context map, then writes
one value to a host-owned output buffer. Providers decode the call with
`decode_invocation`; its `arguments` and `runtime_context` fields use explicit
typed wire values. The wire does not expose Rust layout, a Rust allocator, trait
objects, Bevy values, USD objects, or JSON. The host preserves exact `f64` bits
and validates the returned value against the reflected hook contract before the
owner consumes it.

Provider callbacks must not retain input/output pointers or unwind across the
C ABI. A provider should decode the invocation, use its arguments and context,
perform its computation, and return a value matching the hook's declared
output. It must not mutate ECS or USD directly. The Rust hook owner remains
responsible for validating facts and committing the resulting generic action or
data plan.

## Terrain and co-simulation extension point

The generic loader is ready for domain providers, but it does not invent a
terrain hook. A terrain contract must first be declared and consumed by its
authoritative owner. The appropriate boundary is the expensive pure
`lunco-terrain-bake` provider/kernel seam: it may eventually accept a typed
`DemBakeJob`-like request or a host-owned asset handle and return a validated
`HeightGrid`-compatible result. The provider can then implement that generic
contract with Chrono or another native library without adding a Chrono branch
to the simulator.

`lunco-terrain-core` remains the projection-independent geometry and LOD
substrate. `lunco-terrain-surface` remains the runtime projection and collider
owner; it is not the native-plugin host. This keeps a future terrain mutation
provider reusable by baking, co-simulation, workers, and other consumers while
preventing a plugin from reaching around USD-authored identity or the runtime
ownership boundary.

The same rule applies to other heavy domains: add a hook only when there is a
real owner and consumer, expose facts through the smallest typed contract, and
keep policy selection in Rhai. A provider is an implementation of a reusable
engine seam, not a new domain-specific dispatch table.

## Discovery and authoring

`list_hooks()` and `DiscoverSchema.hooks` expose the owner, typed parameter
names, output type, determinism, installability, active backend, and policy
binding. The catalog is collected from declarations automatically; application
composition does not maintain a manual list. Native provider capabilities are
validated against this same catalog at load time. Rhai startup policies use the
same reflected ids and can replace an optional native implementation only when
the owning contract permits it.

The native-provider feature is opt-in at the application layer. Ordinary builds
that do not accept Twin native providers do not activate the loader path, while
the hook registry, Rhai policy path, and all low-level typed mechanisms remain
available independently.
