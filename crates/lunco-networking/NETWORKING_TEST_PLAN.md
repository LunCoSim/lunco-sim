# Networking test plan — "is everything we need there?"

A requirement → test → status matrix. Splits into what's **testable now** (pure
logic, no backend) vs. what becomes a **headless integration test** once the
backend is committed, vs. **manual/visual** (browser, latency feel).

Run the pure suite:
```sh
cd crates/lunco-networking/proto-tests
cargo test -j 2            # 23 tests, ~3s, zero deps
```

---

## Tier 1 — backend-agnostic core (AUTOMATED NOW ✅)

Implemented in `proto-tests/`, all green (23 tests). These are the parts no
networking library gives us, so they're the most important to lock down early.

| Requirement (mechanism / gap) | Test(s) | Status |
|---|---|---|
| M1 identity deterministic across processes | `m1_identity_is_deterministic_across_processes` | ✅ |
| M1 id stays in 53-bit JS-safe space | `m1_identity_fits_53_bits` | ✅ |
| M1 namespace isolation (extensibility seam) | `m1_namespace_isolates_identity` | ✅ |
| M1 distinct content → distinct ids | `m1_distinct_paths_distinct_ids` | ✅ |
| M1 path canonicalization stable (cross-platform) | `m1_path_canonicalization_is_stable` | ✅ |
| M1 Authoritative/Local have no derived id | `m1_authoritative_and_local_have_no_derived_id` | ✅ |
| M1 Derived id from (parent, role) | `m1_derived_depends_on_parent_and_role` | ✅ |
| M1 no collisions in realistic sample | `m1_no_collisions_in_realistic_sample` | ✅ |
| Select: driven rover → M2-Predicted | `select_driven_rover_pose_is_predicted` | ✅ |
| Select: cosim body → M2-Interpolated (gap C) | `select_cosim_driven_body_is_interpolated` | ✅ |
| Select: possess/param → M3 | `select_possess_is_command`, `select_runtime_static_is_command` | ✅ |
| Select: intent → M4 | `select_intent_is_input` | ✅ |
| Select: Modelica text → M5 | `select_modelica_text_is_crdt` | ✅ |
| Select: camera → M7 | `select_camera_is_local` | ✅ |
| Select: content structure → M1 | `select_content_static_is_content` | ✅ |
| Select: derived-from-synced → M7 recompute (Step 0.5) | `select_derived_value_recomputed_is_local` | ✅ |
| Enforce: Local must be LocalOnly authority | `contradiction_local_provenance_must_be_localonly` | ✅ |
| Enforce: cannot predict Opaque (gap C) | `contradiction_cannot_predict_opaque` | ✅ |
| Gap A: rebase preserves absolute world | `rebase_preserves_absolute_world_position` | ✅ |
| Gap A: two clients agree on world | `rebase_two_clients_agree_on_world` | ✅ |
| Gap A: world roundtrip stable | `world_roundtrip_is_stable` | ✅ |
| Gap A: offset bounded ⇒ cheap quantization | `offset_normalization_is_bounded` | ✅ |

---

## Tier 2 — backend-dependent (HEADLESS INTEGRATION, after backend committed ⏳)

Once lightyear is in (post-Ph0), use lightyear's **`lightyear_crossbeam`**
in-memory transport to run a server `App` + client `App`(s) in one process,
headless, and step them. No sockets, no browser — fast and CI-able. (This is how
lightyear tests itself.)

Sketch:
```rust
// pseudo — two Apps connected by crossbeam channels, stepped N frames
let mut server = make_app(Mode::Server);
let mut client = make_app(Mode::Client);
connect_crossbeam(&mut server, &mut client);
for _ in 0..60 { server.update(); client.update(); }
assert!(client_has_replicated_entity(&client));
```

| Requirement | Test to add | Method |
|---|---|---|
| **M2** replicated component arrives on client | `replication_reaches_client` | crossbeam, step, assert component present |
| **M2** `(CellCoord,Transform)` both replicate (gap A) | `gridpos_replicates` | assert cell+offset on client |
| **M4** client input mutates server state | `input_mutates_server` | send `DriveRover`, step, assert server pos changed |
| **M2-Predicted** owner entity has Predicted | `owner_is_predicted` | assert `Predicted` marker on client-owned |
| **M2-Interpolated** remote entity interpolates | `remote_is_interpolated` | assert `Interpolated` + buffer fills |
| **Reconciliation** mispredict → smooth correct (gap F) | `mispredict_corrects_without_teleport` | inject divergence, assert bounded correction step |
| **M6** client tick runs ahead of server | `client_tick_leads_server` | assert tick offset ≈ RTT/2 |
| **M3** `Mutation`/`#[Command]` over wire, OpId dedupe | `command_idempotent` | replay same OpId, assert applied once |
| **Host-client** (listen-server) replicates to a joiner | `host_client_replicates` | host App + client App, assert joiner sees host entity |
| Late-join baseline (M1 reload + M2 snapshot) | `late_joiner_converges` | join after N ticks, assert convergence |
| Multi-transport: clients on different transports share world | `mixed_transport_one_world` | two clients, assert both see each other |
| Convergence under loss (M2 self-heals, M3 retransmits) | `converges_under_packet_loss` | conditioner with loss, assert eventual equality |

---

## Tier 3 — manual / visual (per `SPIKE_PH0.md` ▶ human-run 👁)

Can't meaningfully assert in code; verify by eye.

| Requirement | How | Where |
|---|---|---|
| Browser client over WebTransport (+cert) | open Chrome/Edge/FF tab, see predicted+server cubes | SPIKE_PH0 Step A |
| Input feels zero-latency (prediction) | drive, watch own cube respond instantly | SPIKE_PH0 |
| Corrections smooth under ~80–150 ms latency/jitter | run conditioner, watch for teleport snaps | SPIKE_PH0 |
| Host-client robustness (lightyear's rough spot) | host-client mode under latency | SPIKE_PH0 decision gate |
| Floating-origin rebasing looks right at scale | drive far, confirm no jitter as origin rebases | Ph3 |

---

## Coverage summary
- **Now (no backend, no heavy build):** identity, mechanism selection, enforced
  contradictions, big_space math — **23 automated tests, green.**
- **After backend commit:** ~12 headless crossbeam integration tests (replication,
  input, prediction, reconciliation, clock, host-client, loss, multi-transport).
- **Manual:** browser/WebTransport + latency feel (5 checks) — inherently visual.

The Tier-1 suite already answers "do we have the hard, backend-agnostic pieces
right?" — yes. Tier 2 answers "does the chosen backend deliver the wire mechanisms?"
and is written the moment Ph0's decision gate picks lightyear.
