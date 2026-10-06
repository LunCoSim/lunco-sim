---
name: networking-deployment
description: Configure, smoke-test, or deploy LunCoSim networking, the headless server, WebTransport, multiplayer sessions, TLS, or host/client launch. Use for remote simulation, networking feature builds, firewall, service, and deployment questions.
---

# Networking and deployment

Read [`crates/lunco-networking/DEPLOY.md`](../../crates/lunco-networking/DEPLOY.md),
the networking [`README.md`](../../crates/lunco-networking/README.md), and the
[synchronization contract](../../crates/lunco-networking/SYNC_ARCHITECTURE.md#mounted-scene-and-connection-lifetime) before editing. `lunco-networking` owns
transport/session behavior and `lunco-networking-sync` owns the transport-neutral
replication runtime; the deployment guide owns service and TLS facts.

## Local validation

- Build the required production target with the documented `networking` feature
  and use the resulting binary; do not infer network behavior from a GUI-only
  build.
- Networking is explicit on every launcher: use `--host` or `--connect` only
  for an authorized network session. Headless desktop, scene tests and the
  dedicated launcher stay local otherwise. Follow the
  [startup and endpoint admission contract](../../docs/apps/luncosim/OPS.md#3-run-the-server).
  Cover malformed/conflicting CLI flags and browser overrides with the generic
  `network_mode_configuration_`, `network_endpoint_configuration_`, and
  `network_endpoint_socket_family_` tests.
  The Windows nightly job runs the exact network admission regressions,
  including unpaired native argument surrogates and drive/UNC TLS paths.
  Prove public `JoinServer` rejection preserves the prior connection through
  the production command surface; an unresolvable valid hostname is a transport
  failure after admission, rather than a syntax rejection.
- Use [`scripts/net_smoke.sh`](../../scripts/net_smoke.sh) or
  [`scripts/run_host_client.sh`](../../scripts/run_host_client.sh) for the
  narrowest real host/client check. Give every controllable process an explicit
  free API port and clean up through the API `Exit` command.
- Keep `--api` local unless the deployment contract explicitly requires a
  tunnel or authenticated remote boundary. Never expose the admin API merely
  to make WebTransport work.
- Manifest admission captures indexed paths and `FileClosureLimits`; the
  existing task performs dependency reads, re-rooting and hashing. An exact
  still-active Twin build failure surfaces through `NetStatus.last_error` and
  its runtime diagnostic, without publishing a partial manifest. Cover
  `scenario_manifest_preparation_rejects_missing_documents_and_invalid_limits`
  and the generic asset closure budget/error seam. See the
  [closure contract](../../docs/architecture/16-document-identity-and-collaboration.md#dependency-closure-separates-asset-traversal-from-usd-interpretation).

- Check exact transport and mount admission at both the wire producer and its
  real scene/document consumer. Preserve typed `ReplicationScope` internally;
  use the protocol adapter only at serialization. Application is valid only in
  a scene-free world; missing Twin ownership must reject visibly.
- Cache persistence uses the [storage atomic-write contract](../../crates/lunco-storage/README.md#usage). Cover concurrent replacement at that generic owner before relying on detached cache writes.
- Revision and catalog work follows the [cache owner contract](../../crates/lunco-networking/SYNC_ARCHITECTURE.md#revision-cache-and-catalog). Pass `ScenarioCacheLimits.max_record_bytes` to native/OPFS `read_bounded` before decoding metadata. Cover UUID/revision isolation, conflicting same-name mounts, out-of-order metadata completion, retention, corrupt/oversized records, and the OPFS configuration with generic temp fixtures.
- For lifecycle work, cover old-mount rejection, same-source reopening, exact
  connection replacement, deferred spawn/replay retirement, asynchronous result
  cancellation, and prediction-buffer teardown at their generic owners. The
  scene-free `net_smoke` harness covers Application traffic; it does not prove
  Twin reload behavior. Use an owned host/client session for that acceptance.
- Preserve `SyncInboxEntry` connection provenance through deferral and retry.
  Host journal replay reads `JournalIngressOrigins` rather than guessing an
  owner from retained history. Run status requires the immutable experiment
  origin and current admitted owner. Cover EntryId reuse and exact
  `ReplicationOwnerRetired` teardown at the generic transport seam.

## Production deployment

Host admission validates native TLS paths, PEM/key matching, bind restrictions,
and netcode keys before creating a listener. Invalid explicit configuration
rejects visibly through `NetStatus.last_error`; check listener admission rather
than process liveness alone. `JoinServer` rejects invalid endpoint/key/pin configuration
before replacing the current connection. Pins cross the transport boundary as
validated 32-byte values, and the invite digest belongs to the session status.

Follow the deployment guide's exact binary, asset, cache, service-account,
firewall, TLS, and nginx layout. Use a real non-development netcode key before
binding a public interface. Verify service logs, certificate renewal, client
connection, and the authenticated API path. A successful local compile is not
deployment evidence.

Networking policy belongs in the networking/deployment owners and authored
scenario policy remains in Rhai. Do not add a second transport, silently open a
port, or claim multiplayer support from a single-process smoke test.
