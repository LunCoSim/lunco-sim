# OPEN — local API authority remains trusted by design

**Status: network authorization findings closed; local API filesystem authority remains
an explicit deployment boundary.** The host now authenticates and binds network peers,
limits inbound commands to the declared wire surface, assigns roles server-side, gates
script reflection writes, and canonicalizes journal authors against the connection.

## Enforced boundaries

- The host rejects undeclared and `Local` reflected command types before they reach the
  reflection registry (`crates/lunco-networking/src/sync.rs`).
- Native netcode loads `LUNCO_NETCODE_KEY` or `LUNCO_NETCODE_KEY_FILE`. Missing keys use
  a marked development key and force a loopback bind; a public bind requires a real key
  and `LUNCO_NET_BIND`.
- A connecting peer is an authenticated `Observer` with a host-issued credential. A
  profile name updates display metadata only; it cannot grant `Operator`.
- Script component and resource setters use named capabilities through
  `enforce_script_authority`; component fields are ownership-gated and resource fields
  require `Operator`.
- The host rewrites inbound journal entry identity to the connection-bound author sent
  in the handshake. The journal wire version is bumped with that schema change.
- API-requested screenshots are confined to `LUNCO_SCREENSHOT_ROOT` (the process working
  directory by default), including symlink/traversal checks.

## Deliberate local-authoring boundary

The native command API binds to `127.0.0.1` only. `OpenFile`, `OpenFolder`, and
`OpenTwin` intentionally accept paths selected by the local workbench/picker so a user
can open a Twin outside the current workspace. This remains safe only when every local
process/user is trusted; an untrusted local process must not be given the API port.
The API has no user/session authentication, so it must not be exposed through a proxy
or public interface without an authenticated gateway.

The networking smoke test and deployment path must use a real non-development
`LUNCO_NETCODE_KEY` before binding beyond loopback. The old all-zero key and implicit
public development bind are gone.
