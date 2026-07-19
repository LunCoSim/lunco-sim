//! # Control-allocation kernels
//!
//! A **kernel** is the per-tick, deterministic map from a vessel's logical command
//! inputs (its `FlightSoftware` command surface) to its **actuator-port setpoints**.
//! Rover skid/ackermann mixing is the first case; flight attitude/RCS allocation is
//! the same shape (see the TODO on [`ControlKernelRegistry`]).
//!
//! ## Why a registry, not a component-type per steering architecture
//!
//! The old model had a Rust *component* per arch (`DifferentialDrive`, `AckermannSteer`)
//! dispatched by a hardcoded `if/else` in the mix system — a taxonomy that every new
//! behaviour had to edit. Instead a kernel **self-registers by name** into an open
//! registry (the same pattern as `PortRegistry` and `register_commands!`), and USD
//! **selects** it by name. Adding a steering architecture (or a flight allocator) is a
//! new registration + USD data — it touches no central dispatch and adds no component
//! type. The kernel *math* stays Rust (per-tick, replayed by network prediction —
//! mechanism); its *identity, ports, and coefficients* are data.
//!
//! Names mirror Omniverse PhysX Vehicle: `skid` ≈ `PhysxVehicleTankDifferentialAPI`,
//! `linear` ≈ `PhysxVehicleMultiWheelDifferentialAPI` / `AckermannSteeringAPI`.

use bevy::prelude::*;
use std::collections::HashMap;

/// Normalized command inputs a kernel consumes: `throttle`/`steer` in `[-1,1]`,
/// `brake` in `0..1`. The vessel-agnostic command vector, read from the vessel's
/// `FlightSoftware` command surface by the driving system.
#[derive(Debug, Clone, Copy, Default)]
pub struct DriveInputs {
    pub throttle: f64,
    pub steer: f64,
    pub brake: f64,
}

/// One linear mix term: `value = throttle·forward + steer·steer + brake·brake`,
/// clamped to `±1`. Covers ackermann-style drive (`forward=1,steer=0` on drive
/// ports + a dedicated `steer=1` steering port) and arbitrary per-wheel routing.
#[derive(Debug, Clone, Reflect, Default)]
pub struct MixEntry {
    /// FSW actuator-port name this term writes.
    pub port: String,
    pub forward: f64,
    pub steer: f64,
    /// Applied as a `0/1` gate while braking (a brake port gets `brake=1`).
    pub brake: f64,
}

/// A vessel's actuator-allocation spec: which kernel maps its command inputs to
/// actuator ports, plus that kernel's parameters. Authored from USD — the reader
/// selects the kernel from the Omniverse differential/steering schema the asset
/// declares (or an explicit `lunco:driveMix`). Replaces the per-arch component
/// types (`DifferentialDrive`/`AckermannSteer`/`GenericDriveMix`).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component, Default)]
pub struct DriveMix {
    /// Registry key of the allocation kernel — `"skid"`, `"linear"`, and later
    /// `"attitude"`/`"rcs"` for flight.
    pub kernel: String,
    /// Ordered actuator-port names for positional kernels (skid: `[left, right]`).
    pub ports: Vec<String>,
    /// Linear mix terms (the `linear` kernel; empty for positional kernels).
    pub entries: Vec<MixEntry>,
}

impl DriveMix {
    /// A `skid` mix over two drive ports (Omniverse `TankDifferentialAPI`).
    pub fn skid(left: &str, right: &str) -> Self {
        Self {
            kernel: "skid".to_string(),
            ports: vec![left.to_string(), right.to_string()],
            entries: Vec::new(),
        }
    }

    /// A `linear` mix built from the compact `lunco:driveMix` string:
    /// whitespace-separated `port=forward,steer[,brake]` terms (brake defaults 0).
    /// Example: `"drive_left=1,0 drive_right=1,0 steering=0,1"`. Malformed terms
    /// are skipped with a warning.
    pub fn parse_linear(spec: &str) -> Self {
        let mut entries = Vec::new();
        for term in spec.split_whitespace() {
            let Some((port, coeffs)) = term.split_once('=') else {
                warn!("driveMix term '{term}' missing '=port=f,s[,b]'; skipped");
                continue;
            };
            let nums: Vec<f64> = coeffs.split(',').filter_map(|c| c.trim().parse().ok()).collect();
            if port.is_empty() || nums.len() < 2 {
                warn!("driveMix term '{term}' needs port=forward,steer[,brake]; skipped");
                continue;
            }
            entries.push(MixEntry {
                port: port.to_string(),
                forward: nums[0],
                steer: nums[1],
                brake: nums.get(2).copied().unwrap_or(0.0),
            });
        }
        Self { kernel: "linear".to_string(), ports: Vec::new(), entries }
    }

    /// A **scripted (rhai) kernel**: `kernel` names a `lunco_hooks` hook id (the
    /// `lunco:driveKernel` attribute) instead of a built-in registry entry. The
    /// hook computes the per-port `[-1,1]` outputs itself, so `ports`/`entries` are
    /// empty. `apply_drive_mix` falls back to the hook when the name isn't a
    /// registered built-in — the "control policy in rhai" path.
    pub fn scripted(hook_id: &str) -> Self {
        Self { kernel: hook_id.to_string(), ports: Vec::new(), entries: Vec::new() }
    }
}

/// A control-allocation kernel: a **pure** map from command inputs + the vessel's
/// [`DriveMix`] params to **normalized** actuator-port writes (each in `[-1,1]`);
/// the caller scales to the hardware register. A non-capturing `fn` pointer, so the
/// registry is `Copy` and cheap to clone out for `&mut World` access.
pub type ControlKernel = fn(DriveInputs, &DriveMix) -> Vec<(String, f64)>;

/// The open registry of allocation kernels — **the** mechanism for adding actuation
/// behaviours without a central dispatch or a component-type per architecture. A
/// behaviour self-registers by name; USD references it. Same shape as
/// [`crate::ports::PortRegistry`].
///
/// TODO(behaviour-registry): this is the seed of a general "behaviour" system. The
/// endgame is a single abstraction — **a behaviour is a named, data-parameterized,
/// optionally-stateful transform over the port graph: read some ports → write some
/// ports** — of which today's `ControlKernel` (an *allocator*: command ports →
/// actuator ports) is one KIND. The general registry must span the behaviour kinds:
///   1. **Allocators** (command → actuator) — this registry. skid/linear now;
///      flight attitude-mix / RCS-allocation / thrust-vectoring register here as
///      drop-ins (Omniverse PhysX Vehicle is ground-only, so flight allocators are
///      our design, aligned to aerospace GNC rather than Omniverse).
///   2. **Stateful controllers** (setpoint + measurement + state → command) — PID /
///      attitude-rate / descent-hold. Need a per-entity control-state component
///      (integrator/derivative) + gains as USD data. Different fn signature.
///   3. **Couplings / constraints** (state → corrective force/torque) — e.g. the
///      rocker-bogie differential (`lunco_mobility::DifferentialCoupling`): reads
///      two rocker hinge angles, writes equal/opposite torques. Already data-authored
///      (not a taxonomy), but it lives as its own component+system; fold it in as the
///      "coupling" kind so it's a registered, data-selected behaviour like the rest.
///      NOTE: it does NOT fit the `ControlKernel` signature (no command input, reads
///      joint state, writes torque) — hence "different kind", not "same fn".
///   4. **Allocation from actuator geometry** — derive the mix matrix from actuator
///      `position + axis + maxForce` (how Omniverse builds wheel response and how a
///      spacecraft RCS pseudo-inverse is formed) via a generic `lunco:actuator`
///      schema, instead of hand-authored coefficients.
///   5. **Command-layer arbitration** — auto vs. manual writing one command port
///      (priority/blend); the takeover problem.
///
/// Once all kinds read/write via the `PortRegistry`, "behaviour" collapses to one
/// port→port transform concept and this becomes the general mechanism.
#[derive(Resource, Default, Clone)]
pub struct ControlKernelRegistry {
    kernels: HashMap<String, ControlKernel>,
}

impl ControlKernelRegistry {
    /// Register (or replace) a kernel by name. Call from a plugin `build`.
    pub fn register(&mut self, name: &str, kernel: ControlKernel) {
        self.kernels.insert(name.to_string(), kernel);
    }

    /// Look up a kernel by name (the `DriveMix.kernel` key).
    pub fn get(&self, name: &str) -> Option<ControlKernel> {
        self.kernels.get(name).copied()
    }

    /// The built-in rover kernels (`skid`, `linear`). Flight allocators register
    /// additively from their own crate.
    pub fn with_defaults() -> Self {
        let mut r = Self::default();
        r.register("skid", skid_kernel);
        r.register("linear", linear_kernel);
        r
    }
}

// ── Built-in kernels (pure math; live in core as the base behaviour library) ─────

/// Skid / tank-differential kernel (Omniverse `PhysxVehicleTankDifferentialAPI`).
/// `ports = [left, right]`. See [`skid_mix_norm`] for the nonlinearity.
pub fn skid_kernel(cmd: DriveInputs, mix: &DriveMix) -> Vec<(String, f64)> {
    let (l, r) = skid_mix_norm(cmd.throttle, cmd.steer);
    let mut out = Vec::with_capacity(2);
    if let Some(p) = mix.ports.first() {
        out.push((p.clone(), l));
    }
    if let Some(p) = mix.ports.get(1) {
        out.push((p.clone(), r));
    }
    out
}

/// Normalized skid mix: `(forward, steer)` → `(left, right)`, each in `[-1,1]`.
///
/// Two properties a plain linear mix lacks:
///   1. **Steer-priority**: hard steering bleeds off forward authority
///      (`drive = forward·(1 − 0.5·|steer|)`) so the inner side can counter-rotate —
///      otherwise the outer side saturates and steering becomes a lazy arc ("can't
///      steer while driving forward").
///   2. **Proportional saturation**: when the mix exceeds `±1`, both sides scale by
///      the larger magnitude, preserving the commanded L/R ratio instead of clamping
///      each side independently (which discards half the differential).
pub fn skid_mix_norm(forward: f64, steer: f64) -> (f64, f64) {
    let steer = steer.clamp(-1.0, 1.0);
    let drive = forward.clamp(-1.0, 1.0) * (1.0 - 0.5 * steer.abs());
    let l = drive + steer;
    let r = drive - steer;
    let m = l.abs().max(r.abs()).max(1.0);
    (l / m, r / m)
}

/// Linear allocation kernel (Omniverse `PhysxVehicleMultiWheelDifferentialAPI` /
/// `AckermannSteeringAPI` drive). Each [`MixEntry`] is
/// `throttle·forward + steer·steer + brake·brake`, clamped to `±1`.
pub fn linear_kernel(cmd: DriveInputs, mix: &DriveMix) -> Vec<(String, f64)> {
    mix.entries
        .iter()
        .map(|e| {
            let v = cmd.throttle * e.forward + cmd.steer * e.steer + cmd.brake * e.brake;
            (e.port.clone(), v.clamp(-1.0, 1.0))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skid_steer_priority_lets_the_inner_side_counter_rotate() {
        // Full throttle + full steer: drive = 1·(1−0.5) = 0.5, l = 1.5, r = −0.5,
        // m = 1.5 → outer l = 1.0, inner r = −1/3. The inner side going NEGATIVE
        // (not clamped to 0) is the point — it lets the rover pivot under throttle.
        let (l, r) = skid_mix_norm(1.0, 1.0);
        assert!((l - 1.0).abs() < 1e-9, "outer l={l}");
        assert!(r < 0.0 && (r + 1.0 / 3.0).abs() < 1e-9, "inner should counter-rotate, r={r}");
    }

    #[test]
    fn skid_reverse_mirrors_forward_at_zero_steer() {
        // Pure throttle mirrors cleanly (both sides saturate equally, m = 1). With
        // steer ≠ 0 the proportional-saturation divisor differs per direction, so
        // it is deliberately NOT a mirror there.
        let (lf, rf) = skid_mix_norm(1.0, 0.0);
        let (lr, rr) = skid_mix_norm(-1.0, 0.0);
        assert!((lf + lr).abs() < 1e-9);
        assert!((rf + rr).abs() < 1e-9);
    }

    #[test]
    fn skid_kernel_maps_to_named_ports() {
        let mix = DriveMix::skid("drive_left", "drive_right");
        let out = skid_kernel(DriveInputs { throttle: 1.0, steer: -1.0, brake: 0.0 }, &mix);
        assert_eq!(out[0].0, "drive_left");
        assert_eq!(out[1].0, "drive_right");
        // steer = -1 → left counter-rotates, right drives.
        assert!(out[0].1 < 0.0 || out[0].1.abs() < 1e-9);
        assert!(out[1].1 > 0.0);
    }

    #[test]
    fn linear_kernel_projects_each_entry() {
        let mix = DriveMix::parse_linear("drive_left=1,0 drive_right=1,0 steering=0,1");
        let out = linear_kernel(DriveInputs { throttle: 0.5, steer: 0.8, brake: 0.0 }, &mix);
        let get = |n: &str| out.iter().find(|(p, _)| p == n).unwrap().1;
        assert!((get("drive_left") - 0.5).abs() < 1e-9);
        assert!((get("drive_right") - 0.5).abs() < 1e-9);
        assert!((get("steering") - 0.8).abs() < 1e-9);
    }

    #[test]
    fn registry_defaults_resolve_by_name() {
        let reg = ControlKernelRegistry::with_defaults();
        assert!(reg.get("skid").is_some());
        assert!(reg.get("linear").is_some());
        assert!(reg.get("nope").is_none());
    }
}
