# Ph1 — `lunco-core` changes (backend-agnostic patch spec)

> **APPLIED 2026-05-29.** All 5 patches landed in `lunco-core` (+ 5 Bevy-wiring
> tests). Three deviations from the draft below, decided against the *actual* code
> (the draft's snippets had drifted):
> 1. **`fold_53` / separators** — ported the **proto-tests reference verbatim**
>    (`(h ^ (h>>53) ^ (h>>32)) & MASK`, Content joins with byte `:`, Derived with
>    byte `/`), NOT the draft's `(h ^ (h>>53))` / `\u{1f}` form. The 23 green tests
>    are canonical; the draft paraphrase was wrong and would have desynced peers.
> 2. **`from_raw` is `pub`, not `pub(crate)`** — `lunco-api::executor` and
>    `lunco-sandbox-edit` legitimately reconstruct a `GlobalEntityId` from a sync-layer
>    `u64`. That's reconstruction, not minting; locking it crate-private would
>    break the API boundary. Minting is still closed (`new()`/`Default` removed).
> 3. **`advance_sim_tick` reads `Option<Res<TimeWarpState>>`** — `TimeWarpState`
>    isn't guaranteed inserted by every binary; optional read avoids a panic and
>    defaults to "running".
>
> Migration: used the **safe/incremental** path (untagged entity → auto-allocate +
> warn-once), so nothing breaks day one. Blast radius: 3 real construction sites
> (`executor.rs` ×2, `sandbox-edit/commands.rs`) + 2 test sites
> (`lunco-api/registry.rs`) flipped to `from_raw`, **plus one the grep-audit missed**
> — `lunco-api/transports/http.rs:72` `QueryEntity` parsed an id via `FromStr` then
> `.unwrap_or_default()`, relying on the now-removed `Default`; fixed to
> `.unwrap_or(GlobalEntityId::from_raw(0))` (a non-resolving sentinel — the old
> `Default` minted a random, equally non-matching id).
>
> **BUILT GREEN 2026-05-29** (`-j2`): `cargo test -p lunco-core` → all 5 Ph1 tests
> pass (+ existing 14+2); `cargo check -p lunco-api -p lunco-sandbox-edit` clean.


Concrete patch for the parts **no networking library gives us**: deterministic
identity (M1), enforced-by-design provenance, and the replicated discrete
**sim-tick** (M6 substrate). All of this compiles and is testable **without** any
backend committed — it's safe to land before/independently of the Ph0 decision.

This is a **spec**, not applied edits — `lunco-core` is a deep dependency and
rebuilding the workspace is expensive on this machine. Apply when ready.

Grounded in the current code (2026-05-29):

| Thing | Where it is now |
|---|---|
| `GlobalEntityId(pub u64)` + `new()`/`Default` | `lunco-core/src/lib.rs:64-93` |
| `assign_global_entity_ids()` (PostUpdate auto-assign) | `lunco-core/src/lib.rs:250-258` |
| `make_id_53()` (time+random, **non-deterministic**) | `lunco-core/src/ids.rs:23-50` |
| `SyncChannel{Local,CommandBus,ControlStream}` (renamed from `Replication`) | `lunco-core/src/commands.rs` |
| `Mutation<P>`,`OpId`,`SessionId` | `lunco-core/src/commands.rs` |
| `TimeWarpState`, `CelestialClock` | `lunco-core/src/lib.rs:168-223` |
| observer idiom `On<Add, T>` + `trigger.entity` | `lunco-cosim/src/lib.rs:77-142` |
| `Time::<Fixed>::from_hz(60.0)` | `lunco-client/src/bin/sandbox.rs:153` |

The proto-tests in `proto-tests/` are the **reference implementation** of the
identity + classification logic below — this spec ports them into `lunco-core`
with Bevy types attached. Logic is already green (23 tests).

---

## The core problem this fixes

`GlobalEntityId::new()` → `make_id_53()` mixes **wall-clock seconds + randomness**.
Two processes loading the same USD prim get **different** ids. That breaks M1
(content-reconstruction): clients can't agree on identity without coordination.

The fix is **provenance-driven id assignment**:

- **Content** (loaded from USD/glTF/…): id = deterministic hash of
  `(namespace, source, canonical-path)`. Same everywhere, zero coordination.
- **Derived** (wheel of a rover): id = hash of `(parent-id, role)`.
- **Authoritative** (runtime-born, e.g. spawned projectile): server allocates via
  the existing `make_id_53()` and replicates the id down. (Non-deterministic is
  *correct* here — only the server mints it.)
- **Local** (camera, hover highlight): **never gets a GlobalEntityId at all**.

Enforced by design: you can't get a `GlobalEntityId` without a `Provenance`, and
the single assignment system is the only place ids are minted.

---

## Patch 1 — new module `lunco-core/src/identity.rs`

Port `proto-tests/src/identity.rs`, adding the Bevy `Component`. Deterministic
hash is **FNV-1a 64 → folded to 53 bits** (NOT `DefaultHasher` — that's not
cross-platform stable). Keep folding identical to proto-tests so the 23 tests
stay valid.

```rust
//! Provenance-driven deterministic identity (M1). See SYNC_ARCHITECTURE.md.
use bevy::prelude::*;

/// 53-bit mask — ids stay losslessly representable in a JS `Number`.
pub const ID_MASK_53: u64 = (1u64 << 53) - 1;

/// Where an entity's identity *comes from*. Required to obtain a GlobalEntityId.
#[derive(Component, Clone, Debug, PartialEq, Eq, Hash, Reflect)]
#[reflect(Component)]
pub enum Provenance {
    /// Reconstructed identically on every peer from loaded content.
    Content { namespace: String, source: String, path: String },
    /// Deterministically derived from a parent entity + a stable role.
    Derived { parent: u64, role: String },
    /// Runtime-born; only the server mints the id, then replicates it.
    Authoritative,
    /// Never networked, never gets a GlobalEntityId.
    Local,
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Fold 64 bits into the 53-bit JS-safe space (xor high 11 bits down).
pub fn fold_53(h: u64) -> u64 {
    (h ^ (h >> 53)) & ID_MASK_53
}

/// Canonicalize a content path: forward slashes, no doubled/trailing slash.
pub fn canonicalize_path(p: &str) -> String {
    let unified = p.replace('\\', "/");
    let mut out = String::with_capacity(unified.len());
    let mut prev_slash = false;
    for c in unified.chars() {
        if c == '/' {
            if !prev_slash { out.push(c); }
            prev_slash = true;
        } else {
            out.push(c);
            prev_slash = false;
        }
    }
    let trimmed = out.trim_end_matches('/');
    if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() }
}

/// The only deterministic id derivation. `None` for Authoritative/Local
/// (those are minted by the server / never minted).
pub fn derive_id(p: &Provenance) -> Option<u64> {
    match p {
        Provenance::Content { namespace, source, path } => {
            let key = format!("{namespace}\u{1f}{source}\u{1f}{}", canonicalize_path(path));
            Some(fold_53(fnv1a64(key.as_bytes())))
        }
        Provenance::Derived { parent, role } => {
            let mut buf = parent.to_le_bytes().to_vec();
            buf.push(0x1f);
            buf.extend_from_slice(role.as_bytes());
            Some(fold_53(fnv1a64(&buf)))
        }
        Provenance::Authoritative | Provenance::Local => None,
    }
}

/// Test/ergonomic helper.
pub fn content(namespace: &str, source: &str, path: &str) -> Provenance {
    Provenance::Content {
        namespace: namespace.into(), source: source.into(), path: path.into(),
    }
}
```

---

## Patch 2 — lock down `GlobalEntityId` (`lunco-core/src/lib.rs`)

Make the field private and remove the public `new()`/`Default` so ids can only be
minted by the assignment system. (Serde still works — it constructs the field
directly within the crate.)

```rust
// REPLACES lib.rs:64-93
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect,
         serde::Serialize, serde::Deserialize)]
#[reflect(Component)]
pub struct GlobalEntityId(u64);          // field now PRIVATE

impl GlobalEntityId {
    /// Read the raw id (e.g. to put on the sync layer).
    pub fn get(&self) -> u64 { self.0 }

    /// Server-only mint for Authoritative entities. Crate-internal: the
    /// assignment system is the sole caller. Wraps the existing make_id_53().
    pub(crate) fn allocate_authoritative() -> Self { Self(crate::ids::make_id_53()) }

    /// Reconstruct from a sync-layer/derived value. Crate-internal.
    pub(crate) fn from_raw(v: u64) -> Self { Self(v) }
}
// NOTE: no pub `new()`, no `Default`. Anything that did `GlobalEntityId::new()`
// or `GlobalEntityId(x)` must instead attach a `Provenance` and let the
// assignment system mint the id. (Grep first — see "Migration" below.)
```

---

## Patch 3 — provenance-aware assignment (replaces `assign_global_entity_ids`)

The single mint point. Replaces `lib.rs:250-258`. Runs in `PostUpdate` as before.

```rust
/// The ONLY place GlobalEntityIds are minted. Provenance decides how.
fn assign_global_entity_ids(
    mut commands: Commands,
    q: Query<(Entity, &Provenance), Without<GlobalEntityId>>,
    is_server: Res<IsServer>,          // see Patch 5; trivial bool resource
) {
    for (e, prov) in &q {
        match prov {
            Provenance::Local => { /* never networked, no id */ }
            Provenance::Content { .. } | Provenance::Derived { .. } => {
                if let Some(id) = identity::derive_id(prov) {
                    commands.entity(e).insert(GlobalEntityId::from_raw(id));
                }
            }
            Provenance::Authoritative => {
                // Only the server mints; clients receive the id via replication.
                if is_server.0 {
                    commands.entity(e).insert(GlobalEntityId::allocate_authoritative());
                }
            }
        }
    }
}
```

**Enforced-by-design consequences:**
- No `Provenance` ⇒ no `GlobalEntityId`. (Today every entity silently got one;
  now identity is opt-in and typed. This is the behavioral change to plan for —
  see Migration.)
- `Local` entities are structurally un-networkable (camera, gizmos).
- Deterministic ids need no server round-trip; only `Authoritative` does.

---

## Patch 4 — discrete replicated **sim-tick** (M6 substrate)

`TimeWarpState`/`CelestialClock` give *continuous* sim time + warp. Netcode also
needs a **monotonic discrete tick** counter — the unit prediction, rollback,
input-stamping, and the shared clock all key off. This is the piece missing
today. Add to `lunco-core/src/lib.rs` (or a small `clock.rs`):

```rust
/// Monotonic 60 Hz simulation tick. The netcode time substrate (M6).
/// Replicated; clients run their copy *ahead* of the server by ~RTT/2 so their
/// inputs arrive on time (lightyear's sync, or hand-rolled with replicon).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq,
         serde::Serialize, serde::Deserialize, Reflect)]
#[reflect(Resource)]
pub struct SimTick(pub u64);

impl SimTick {
    pub fn wrapping_diff(self, other: SimTick) -> i64 {
        self.0.wrapping_sub(other.0) as i64
    }
}

/// Advance once per FixedUpdate. Pause/warp already gate physics via
/// TimeWarpState; the tick advances whenever physics steps. Keep the tick
/// integer and warp-independent (warp scales dt, not tick count) so peers can
/// compare ticks directly.
fn advance_sim_tick(mut tick: ResMut<SimTick>, warp: Res<TimeWarpState>) {
    if warp.physics_enabled && warp.speed > 0.0 {
        tick.0 = tick.0.wrapping_add(1);
    }
}
// register: app.init_resource::<SimTick>()
//           .add_systems(FixedUpdate, advance_sim_tick); // before physics set
```

> Open question for Ph3/Ph4 (note, don't decide now): under time-warp the tick
> advances at warp-scaled wall-rate but stays 1-per-fixed-step in sim terms.
> Confirm lightyear's `Tick` can be driven from our `SimTick` rather than its own
> wall-clock tick, or we keep them parallel and map. Affects M6↔backend seam only.

---

## Patch 5 — `IsServer` role resource (tiny, backend-agnostic)

Assignment (Patch 3) needs to know if this process mints Authoritative ids. One
bool, set once at startup; the backend layer (Ph1+) owns *how* it's set.

```rust
#[derive(Resource, Clone, Copy, Debug)]
pub struct IsServer(pub bool);
impl Default for IsServer { fn default() -> Self { Self(true) } } // single-process = authoritative
```

Single-process today ⇒ `true` ⇒ behavior matches current (everything gets an id),
except `Local`-tagged entities now correctly opt out and Content/Derived become
deterministic. Host-client: host = `true`. Pure client: `false`.

---

## Migration / blast radius (the part to scope before applying)

The behavioral change — *no `Provenance` ⇒ no `GlobalEntityId`* — is the only
risky bit. Two ways to stage it:

- **Safe/incremental (recommended):** keep a fallback arm in `assign_global_entity_ids`
  — entities with no `Provenance` get `Authoritative`-style allocation (current
  behavior), and emit a `warn!` once. Migrate spawners to attach `Provenance`
  over time; flip the fallback to a hard skip once clean. Zero day-one breakage.
- **Strict:** require `Provenance` immediately; grep every `GlobalEntityId::new()`
  / `GlobalEntityId(` / spawner and attach provenance up front.

Before applying, grep to size it:
```sh
rg -n 'GlobalEntityId::(new|default)|GlobalEntityId\(' crates/ --type rust
rg -n 'GlobalEntityId' crates/ --type rust | wc -l
```
Recommend the **safe/incremental** path — it lands the machinery (deterministic
ids, Local opt-out, sim-tick) with no regressions, and the strictness comes later
as spawners are migrated. This keeps the expensive rebuild to **one** pass.

---

## Tests to add alongside (port from proto-tests, now with Bevy)

The 23 proto-tests already validate the pure logic. In `lunco-core`, add a thin
layer proving the **Bevy wiring**:

| Test | Asserts |
|---|---|
| `content_entity_gets_deterministic_id` | spawn `Provenance::Content`, run schedule, `GlobalEntityId::get()` == `derive_id` |
| `local_entity_gets_no_id` | spawn `Provenance::Local`, no `GlobalEntityId` after PostUpdate |
| `authoritative_minted_only_on_server` | `IsServer(false)` ⇒ no id; `IsServer(true)` ⇒ id present |
| `derived_id_matches_parent_role` | spawn parent Content + child Derived, child id == `derive_id(Derived{parent_id,role})` |
| `sim_tick_advances_under_run_paused_does_not` | step FixedUpdate, assert tick moves; pause via TimeWarp, assert frozen |

These run with a headless `App` (no rendering, no backend) — cheap and CI-able.

---

## What this unblocks

After Patches 1–5 land, the backend layer (Ph1+ lightyear app) only has to:
1. replicate `GlobalEntityId` + `Provenance::Authoritative` ids server→client,
2. drive/sync `SimTick`,
3. set `IsServer`.

Everything identity- and tick-shaped is already correct and tested **without** a
backend — which is the whole point of doing this in parallel with the Ph0 spike.
