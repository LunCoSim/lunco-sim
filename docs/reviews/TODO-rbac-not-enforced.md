# TODO — LunCoSim does not enforce access control

**Status: accepted, by design, for now.** This is a deliberate decision, not an oversight.
It is written down so that nobody "discovers" it again in a future audit and so that nobody
deploys the current build somewhere it does not belong.

The 2026-07-12 full code review ([`2026-07-12-full-code-review.md`](2026-07-12-full-code-review.md))
found seven authorization defects. All the *other* findings in that review have been fixed.
These have not been, and will not be until we decide to enforce RBAC.

## The operating assumption

**Every peer that can reach the wire, and every process that can reach the local API, is trusted.**

Concretely, that means LunCoSim today is safe to run:

- on `localhost`,
- on a trusted LAN with trusted participants,
- in a workshop / classroom / demo where every player is in the room.

It is **not** safe to expose a host to an untrusted network. A public `0.0.0.0` bind is
remote code-equivalent execution on the host machine — see `S1` + `S2` below.

## What is not enforced

| id | what | where |
|---|---|---|
| `S1` | Any connected peer can trigger **any** of the 212 reflected command types on the host. Inbound envelopes resolve `type_name` against the whole `AppTypeRegistry`; the `SyncChannelRegistry` allowlist is consulted only on the SEND side, and `CommandPolicy::OPEN` is the default for anything unregistered. `Exit`, `SetShaderSource`, `DeleteShader`, `ApplyUsdOp` are all reachable. | `crates/lunco-networking/src/sync.rs:875-975`; allowlist at `shared.rs:173-177`; policy default at `lunco-core/src/session.rs:757-768` |
| `S2` | The netcode private key is `[0u8; 32]`, so connect-token authentication is nil — anyone can mint a valid token for any host. `setup_host` binds `0.0.0.0`. | `crates/lunco-networking/src/shared.rs:13-14`; bind at `server.rs:339` |
| `S4` | Scripts can write any component or resource field with **no** authorization check. The structural verbs (`add_component`, `remove_component`, `despawn_entity`) *are* gated via `enforce_script_authority`; the field setters are not. | `crates/lunco-scripting/src/bridge_core.rs:752, 791` (vs the gated `:852, 887, 911`) |
| `S5` | The `Operator` role is self-granted: sending `UpdateProfile{name}` promotes `Observer → Operator`. The `AuthorityRole::satisfies` lattice is well-written, well-tested, and enforces nothing. | `crates/lunco-networking/src/sync.rs:2657-2684` |
| `S6` (part) | `CaptureScreenshot.path` and the `Open*` commands take an unvalidated filesystem path over the local API — arbitrary file write and read. **Deliberately left open: the MCP/agent screenshot workflow depends on writing to caller-chosen paths.** | `crates/lunco-api/src/executor.rs:483-498, 706`; `lunco-workbench/src/file_ops.rs:91, 104, 120` |
| `S7` | Inbound `JournalEntry.id.author` is trusted from the wire, and `JOURNAL_EDIT` resolves to `OPEN`. A spoofed author additionally **suppresses the victim's own edits** from ever being relayed (they are filtered on `author != me`). | `crates/lunco-networking/src/sync.rs:1467-1493`; `journal_plane.rs:136-141, 185` |

The `S6` **OOM** half — `SpawnDemTerrain.target_res` was unclamped, so `target_res: 100000`
requested a 100k×100k vertex grid — **has been fixed**, because that is input validation, not
access control, and it is reachable by accident.

## What it would take to enforce

The pieces already exist and are individually well built; nothing here is a rewrite.

1. **Gate the wire on the routing registry.** `SyncChannelRegistry` already *is* the intended
   wire surface. Reject in `apply_sync_command` any `type_name` whose entry is missing or `Local`.
   One `if`.
2. **Flip `CommandPolicyRegistry` to deny-by-default for wire-origin commands** and register the
   intended set explicitly. Today `unregistered_command_is_open_by_default` is an asserted test.
3. **Load the netcode key from env/keyfile**, and refuse to bind a non-loopback address while the
   dev key is in use — fail loud at startup.
4. **Assign roles server-side at connect**, never from a client-supplied display name.
5. **Rewrite `entry.id.author`** against the connection-bound sender identity before `append_remote`.
   Note that netcode identity is *already* correctly connection-bound (`server.rs:870-891`) — the
   plumbing is right, it is just not consulted here.
6. **Route the script field setters through `enforce_script_authority`**, as the structural verbs
   already do.

Item 1 alone closes the largest hole.
