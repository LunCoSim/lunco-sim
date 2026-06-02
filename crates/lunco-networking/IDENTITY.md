# Identity contract — deterministic by provenance, enforced by design

Generalizes the rule "USD-instanced = deterministic id + local spawn" into a single
law that holds for **any** content source (USD today; glTF scenes, procedural
generators, future formats tomorrow) and is **enforced by the type system + ECS
hooks**, not by convention.

> **The law:** an entity's network identity is a pure function of its
> **provenance**. Deterministic derivation is the *default*; server allocation is
> the rare exception, reserved for entities that are genuinely born at runtime and
> cannot be derived from shared content.

If two peers load the same content, they independently arrive at the **same ids**
with zero coordination. Identity stops being "who spawned it, when" and becomes
"what is it, and where did it come from."

---

## 1. Why deterministic-by-default (it pays off far beyond networking)

| Benefit | Mechanism |
|---|---|
| Peers converge without spawn-replication | same content → same id |
| Save / reload is stable | ids survive process restarts |
| Deterministic tests & replay | no random ids to mock |
| Edit-log / collab merge | ops reference content-stable ids |
| USD/glTF round-trip | id ↔ prim path is recoverable |

Random/time-based ids (today's `make_id_53`) give us *none* of these. We keep
time-ordering where it actually belongs — on **operations** (`OpId`), which is a
separate concern from entity identity (`GlobalEntityId`). Don't conflate them.

---

## 2. Provenance taxonomy (a small, closed set — this is the extensibility seam)

```rust
/// Where an entity came from. THE required input to identity.
#[derive(Component, Clone, Debug)]
pub enum Provenance {
    /// Instantiated from shared, content-addressed source data that every peer
    /// loads identically. Id = deterministic hash of (namespace, source, path).
    /// Spawned LOCALLY on each peer; spawn is NOT replicated (only state is).
    Content { namespace: SourceNs, source: SourceId, path: ContentPath },

    /// Deterministically created as a sub-part of another entity (rover→wheels,
    /// device→ports). Id = hash(parent_id, role). Follows the parent's fate.
    Derived { parent: GlobalEntityId, role: RolePath },

    /// Genuinely born at runtime, not derivable from shared content
    /// (player-spawned prop, dynamically generated object). Id = SERVER-allocated;
    /// spawn IS replicated to clients.
    Authoritative,

    /// Never crosses the wire — camera, gizmo, selection, preview. No global id.
    Local,
}
```

- **Adding a new content format never touches this enum.** It registers a new
  `SourceNs` (namespace) and a loader that stamps `Provenance::Content`. USD,
  glTF, procedural, network-imported assets — all flow through the same `Content`
  arm with different namespaces.
- `Derived` generalizes the wheels/ports problem: sub-entities get stable ids from
  their parent + role, so the few cross-references that must survive the wire can
  be resolved, while the parent itself may be `Content` or `Authoritative`.

---

## 3. Derivation function (must be cross-platform stable)

```rust
fn derive_id(p: &Provenance) -> Option<GlobalEntityId> {
    match p {
        Provenance::Content { namespace, source, path } =>
            Some(hash53(&[namespace.as_bytes(), b":", source.as_bytes(), b":", path.as_bytes()])),
        Provenance::Derived { parent, role } =>
            Some(hash53(&[&parent.0.to_le_bytes(), b"/", role.as_bytes()])),
        Provenance::Authoritative => None,   // allocated by the authority, not derived
        Provenance::Local => None,           // never networked
    }
}
```

Non-negotiables for `hash53`:
- A **fixed, specified** hash (blake3 / xxhash / FNV over canonical bytes) —
  **never** `std::DefaultHasher` (randomized per-process → would defeat the whole
  point).
- Output truncated to **53 bits** (same JS-safe space as `make_id_53`).
- Inputs canonicalized (normalized path separators, no trailing slashes, stable
  namespace strings) so byte-identical across platforms.
- 53-bit truncation → finite collision chance. Provenance namespacing keeps the
  per-scene population tiny, but **collision handling must be specified** (see open
  questions) rather than ignored.

---

## 4. Enforced by design (you *cannot* opt out)

Convention rots. Make the invariant structural:

1. **`GlobalEntityId` has no public constructor from a raw integer.** It is minted
   only by the identity layer from a `Provenance`, or received from the authority.
   Domain code physically cannot write `GlobalEntityId(rand())`.

2. **`Provenance` is a *required component* of the networked marker.** A `Networked`
   (replicated) entity without `Provenance` fails to construct — Bevy required
   components / an `on_add` hook enforce it.

3. **An `on_add(Networked)` hook is the single assignment point:**
   ```
   Content | Derived  → insert derive_id(provenance)            (any peer, identical)
   Authoritative      → server: allocate + flag for spawn-replication
                        client: leave empty, await replicated id
   Local              → reject (Local must not be Networked)
   ```
   This is the *only* place ids are assigned — no spawn site does it, so no spawn
   site can do it wrong.

4. **Debug-panic on violation** (Authoritative spawned on a client, Provenance
   missing, Local marked Networked). Fail loud in dev, not silently desynced in
   prod.

Net effect: "deterministic id + local spawn for content; server id + replicated
spawn for runtime" isn't a rule people remember — it's the *only path the API
offers*.

---

## 5. Extensibility: the content-loader registry

```rust
/// Each content format owns a namespace and stamps Content provenance.
trait ContentLoader {
    fn namespace(&self) -> SourceNs;
    fn instantiate(&self, source: SourceId, world: &mut World);  // stamps Provenance::Content{..}
}

app.register_content_loader(UsdLoader);     // namespace "usd"
app.register_content_loader(GltfSceneLoader); // namespace "gltf"   (future)
app.register_content_loader(ProcGenLoader);   // namespace "proc"   (future)
```

A new format adds a loader + a namespace string. **Identity, networking,
replication, and the enforcement hook are untouched** — they only ever see
`Provenance`. That is the extensibility the system was asked for: open to new
sources, closed to changes in the identity machinery.

---

## 6. How it wires into the rest

- **Wire-channel policy** (`lunco-core::WireChannel`) is now *derivable* from
  provenance kind: `Content`→state-replicated/local-spawn, `Authoritative`→
  spawn-replicated, `Derived`→follows parent, `Local`→never. One source of truth.
- **`ApiEntityRegistry`** (GlobalEntityId↔Entity) is populated by the same hook —
  no separate bookkeeping.
- **Late-join (gap I)** simplifies: the handshake sends `scene_id` + content
  sources; the client loads them and **derives the same ids locally**, so only
  *dynamic state* and *Authoritative* entities need streaming.
- **`big_space` (gap A)** is unaffected — identity and coordinates are orthogonal;
  a content entity's id is stable even as its cell+offset replicate and rebase.

---

## 7. What changes from today
- `GlobalEntityId` source: `make_id_53()` (random/time) → `derive_id(provenance)`
  for content/derived; server-allocated for authoritative. Lock down its
  constructor.
- Add `Provenance` (in `lunco-core`, next to `GlobalEntityId`/`Mutation`) + the
  enforcement hook.
- USD loader: stamp `Provenance::Content { namespace: "usd", source: stage_id,
  path: prim_path }` instead of relying on the blanket auto-assign observer.
- Keep `OpId` exactly as-is for operation ordering.

---

## 8. Open questions — **RESOLVED 2026-05-29 (see `DECISIONS.md`)**
1. **Collision policy** in 53-bit space → **D3a**: keep 53-bit on the wire +
   debug-time collision check at load. No 128-bit-internal machinery now (per-scene
   populations tiny); revisit only near ~10⁶ entities.
2. **`SourceId` for a stage** → **D3b**: logical scene name for *identity*,
   content-hash for *cache/dedupe*.
3. **Mutable content** → **D3b (confirmed yes)**: path-based id stays stable across
   live edits; only state replicates. That's the whole point of path-based identity.
