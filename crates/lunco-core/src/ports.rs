//! Co-simulation **port substrate** — the FMI/SSP scalar-exchange surface shared
//! by every participant so they all read/write exposed values through ONE path.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica variables,
//! avian rigid-body state, joint angles, the SysML/hardware "nervous system"
//! ports, and (in future) an imported FMU all present as ports, so that wires,
//! the API (`ListPorts` / `GetPort` / `SetPort`), the UI inspector, and every
//! scripting runtime (rhai/python) treat them uniformly — the FMI/SSP contract.
//!
//! ## Why this lives in `lunco-core`
//!
//! Ports are co-sim *substrate*, not an engine or API concern: the wire engine
//! (`lunco-cosim`) runs ON them, the API and scripts merely consume them. Putting
//! the registry here — below every participant — lets each crate **register** its
//! backends downward and **consume** the registry, with nobody depending "up".
//! This is what lets `lunco-scripting` reach ports even though `lunco-cosim`
//! (which owns the avian/joint/Modelica backends) depends ON scripting: both
//! depend down on this module. A future FMU-import or script-defined component is
//! just one more registered backend the wire engine then honours.
//!
//! ## Value model
//!
//! The wire currency is `f64` (continuous Real — what FMI-CS exchanges almost
//! everywhere). Typed backends convert at their own boundary (a `DigitalPort`
//! `i16` register saturates on write, like a real DAC/ADC). We deliberately do
//! **not** model `Bool`/`Enum`/`String` ports until a concrete need appears.
//!
//! ## One registry, four thin operations
//!
//! Every port-bearing backend is one [`PortBackend`] entry (list / read-output /
//! read-input / write-input), registered into the [`PortRegistry`] resource. The
//! four query methods just fold over the registered backends in order, so a new
//! backend is added by **registering** it — no consumer changes. Registration
//! order *is* resolution precedence (first match wins).

use bevy::prelude::*;
use std::collections::HashMap;

/// Direction (causality) of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum PortDirection {
    /// Port receives values from connections.
    In,
    /// Port provides values to connections.
    Out,
    /// Port can both receive and provide values.
    InOut,
}

/// A discovered port: identity, causality, current value.
///
/// Returned by [`PortRegistry::entity_ports`] for listing/introspection. The
/// `value` is a snapshot read at call time; live consumers read through the
/// registry directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Snapshot of the current value.
    pub value: f64,
}

/// Append every `(name, value)` in `map` as a [`PortRef`] of direction `dir`.
/// Helper for map-backed backends (e.g. Modelica `inputs`/`outputs`).
#[inline]
pub fn push_map(out: &mut Vec<PortRef>, map: &HashMap<String, f64>, dir: PortDirection) {
    for (name, value) in map {
        out.push(PortRef { name: name.clone(), direction: dir, value: *value });
    }
}

/// One port-bearing backend, expressed as four operations over `(World, Entity)`.
///
/// Ops are plain `fn` pointers (non-capturing closures), so a backend is `Copy`
/// and the registry is cheap to clone out of the world for `&mut World` access.
/// Each op is causality-correct: `read_output`/`read_input` see only the matching
/// direction; `write_input` accepts only an existing input slot (the strictness
/// that lets `propagate` report dangling wires). A single-value backend is
/// bidirectional — its one scalar *is* both its output and input.
#[derive(Clone, Copy)]
pub struct PortBackend {
    /// Append this backend's ports on `entity` (outputs then inputs) to `out`.
    pub list: fn(&World, Entity, &mut Vec<PortRef>),
    /// Read the **output** named `name`, or `None`.
    pub read_output: fn(&World, Entity, &str) -> Option<f64>,
    /// Read the **input** named `name`, or `None`.
    pub read_input: fn(&World, Entity, &str) -> Option<f64>,
    /// Write `value` to **input** `name`; `true` iff the port existed here.
    pub write_input: fn(&mut World, Entity, &str, f64) -> bool,

    // ── Optional resolve→slot fast path (the FMI valueReference model) ──────────
    //
    // A backend behind a multi-group presence scan (avian: up to 6 `world.get`
    // gating checks + a name scan per read) can expose these so a hot consumer
    // (the propagation master) resolves an endpoint to a process-local `slot`
    // ONCE (when wiring changes) and then exchanges by slot every tick — one
    // component read, no cross-backend fold, no group scan. `None` ⇒ no fast
    // path; the resolver falls back to the name-based ops above (correct for a
    // map-backed backend registered first, whose name read already costs one
    // `get`). See [`PortRegistry::resolve_output`].
    /// Resolve an **output** name to a backend-private `slot` (opaque `u64`),
    /// or `None` if this backend doesn't own it. Encodes causality: only an
    /// `Out`/`InOut` port resolves here.
    pub resolve_output: Option<fn(&World, Entity, &str) -> Option<u64>>,
    /// Resolve an **input** name to a backend-private `slot`, or `None`. Only an
    /// `In`/`InOut` port resolves here.
    pub resolve_input: Option<fn(&World, Entity, &str) -> Option<u64>>,
    /// Read the value at a previously-resolved `slot`. `None` if the slot no
    /// longer backs a live value (component removed) → the caller re-resolves or
    /// skips, exactly as an absent name read would.
    pub read_slot: Option<fn(&World, Entity, u64) -> Option<f64>>,
    /// Write `value` to a previously-resolved input `slot`; `false` if it no
    /// longer backs a live input.
    pub write_slot: Option<fn(&mut World, Entity, u64, f64) -> bool>,
}

/// A process-local resolved locator for one port on one backend — the FMI
/// *valueReference* analogue.
///
/// `slot` is an opaque `u64` the **owning backend** encodes and decodes; it is
/// meaningful only within this process/run and MUST NEVER be serialized or sent
/// on the wire (resolve fresh on every peer — slots are process-local, like FMI
/// value references). Produced by [`PortRegistry::resolve_output`] /
/// [`resolve_input`](PortRegistry::resolve_input), consumed by
/// [`read_resolved`](PortRegistry::read_resolved) /
/// [`write_resolved`](PortRegistry::write_resolved): the resolver folds over
/// backends ONCE, then the hot loop exchanges by slot with no re-scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPort {
    /// Index of the owning backend in the registry (its registration order).
    backend: usize,
    /// Backend-private opaque locator.
    slot: u64,
}

/// The single registry of port-bearing backends — **the** read/write/list surface
/// for every exposed simulation value, whichever backend owns it.
///
/// Backends are registered (in dependency-correct order) by their owning crate's
/// plugin; the four query methods fold over them. Registration order is
/// resolution precedence (first match wins). `Clone` is cheap (a `Vec` of `Copy`
/// `fn` pointers) so a `&mut World` caller clones it out before writing.
#[derive(Resource, Default, Clone)]
pub struct PortRegistry {
    backends: Vec<PortBackend>,
}

impl PortRegistry {
    /// Register a backend. Later registrations have lower precedence on name
    /// collisions. Call from a plugin `build`.
    pub fn register(&mut self, backend: PortBackend) {
        self.backends.push(backend);
    }

    /// Enumerate every exposed port on `entity`, across all backends.
    /// The backbone of `ListPorts`.
    pub fn entity_ports(&self, world: &World, entity: Entity) -> Vec<PortRef> {
        let mut out = Vec::new();
        for backend in &self.backends {
            (backend.list)(world, entity, &mut out);
        }
        out
    }

    /// Read the **output** named `name` on `entity` — the value a connection reads
    /// from its *source*. Searches outputs only (plus bidirectional single-value
    /// ports). Critical when a name exists as both input and output on one entity.
    pub fn read_output_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_output)(world, entity, name))
    }

    /// Read the current value of port `name`, preferring an **output**, then
    /// falling back to an **input**. The backbone of `GetPort`.
    pub fn read_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        if let Some(v) = self.read_output_port(world, entity, name) {
            return Some(v);
        }
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Read the **input** value of port `name` — the commanded side, skipping
    /// outputs. Use where the input specifically is wanted (e.g. a joint's
    /// commanded motor setpoint vs its measured angle, both named `angle`).
    pub fn read_input_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Write `value` to **input** port `name`. Returns `true` if such an input
    /// existed and was written. Strict: an undeclared name is rejected (never
    /// silently created) — what lets the API and propagation master report
    /// dangling wires. First backend that owns the port wins.
    pub fn write_port(&self, world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
        for backend in &self.backends {
            if (backend.write_input)(world, entity, name, value) {
                return true;
            }
        }
        false
    }

    // ── Resolve→slot fast path ─────────────────────────────────────────────────

    /// Resolve an **output** endpoint `(entity, name)` to a [`ResolvedPort`] — a
    /// process-local handle a hot consumer caches once and reads by slot every
    /// tick. Returns `None` when the precedence-winning owner has no fast path
    /// (the caller then falls back to [`read_output_port`](Self::read_output_port),
    /// which honours the same precedence).
    ///
    /// **Precedence-correct:** walks backends in registration order and stops at
    /// the FIRST that owns `name` — so a lower-precedence fast-path backend can
    /// never shadow a higher-precedence name-only owner (e.g. an avian output
    /// can't win over a `SimComponent` output of the same name on one entity).
    /// The first owner is used whether via slot (it has a fast path) or via the
    /// name read (it doesn't → `None` here).
    pub fn resolve_output(&self, world: &World, entity: Entity, name: &str) -> Option<ResolvedPort> {
        for (i, b) in self.backends.iter().enumerate() {
            // A readable output reveals ownership by name (outputs are readable).
            if (b.read_output)(world, entity, name).is_some() {
                let slot = (b.resolve_output?)(world, entity, name)?;
                return Some(ResolvedPort { backend: i, slot });
            }
        }
        None
    }

    /// Resolve an **input** endpoint to a [`ResolvedPort`] for writing. See
    /// [`resolve_output`](Self::resolve_output).
    ///
    /// Inputs may be **write-only** (an avian `force_y` reads `None`), so a
    /// readable-input probe can't detect every owner. We therefore stop at the
    /// first backend that owns the input *either* readably *or* via its own
    /// `resolve_input` (the authority for write ownership). Precedence holds for
    /// our registration order — the only readable-input backends (`SimComponent`,
    /// FSW) precede the write-only fast-path one (avian) — so a write-only port's
    /// name can't shadow an earlier readable input.
    pub fn resolve_input(&self, world: &World, entity: Entity, name: &str) -> Option<ResolvedPort> {
        for (i, b) in self.backends.iter().enumerate() {
            if (b.read_input)(world, entity, name).is_some() {
                // Earlier readable owner: use its slot if it has a fast path, else
                // `None` → the caller's name write hits it first (precedence held).
                let slot = (b.resolve_input?)(world, entity, name)?;
                return Some(ResolvedPort { backend: i, slot });
            }
            if let Some(resolve) = b.resolve_input {
                if let Some(slot) = resolve(world, entity, name) {
                    return Some(ResolvedPort { backend: i, slot });
                }
            }
        }
        None
    }

    /// Read the value at a resolved port. `None` if the slot no longer backs a
    /// live value (e.g. its component was removed) — the caller skips or
    /// re-resolves, exactly as an absent name read would contribute nothing.
    pub fn read_resolved(&self, world: &World, entity: Entity, r: ResolvedPort) -> Option<f64> {
        (self.backends[r.backend].read_slot?)(world, entity, r.slot)
    }

    /// Write to a resolved input port. `false` if the slot no longer backs a live
    /// input (component removed) — the caller reports the dangling target.
    pub fn write_resolved(&self, world: &mut World, entity: Entity, r: ResolvedPort, value: f64) -> bool {
        match self.backends[r.backend].write_slot {
            Some(write) => write(world, entity, r.slot, value),
            None => false,
        }
    }
}
