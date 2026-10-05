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
- Use [`scripts/net_smoke.sh`](../../scripts/net_smoke.sh) or
  [`scripts/run_host_client.sh`](../../scripts/run_host_client.sh) for the
  narrowest real host/client check. Give every controllable process an explicit
  free API port and clean up through the API `Exit` command.
- Keep `--api` local unless the deployment contract explicitly requires a
  tunnel or authenticated remote boundary. Never expose the admin API merely
  to make WebTransport work.

- Check exact transport and mount admission at both the wire producer and its
  real scene/document consumer. Preserve typed `ReplicationScope` internally;
  use the protocol adapter only at serialization. Application is valid only in
  a scene-free world; missing Twin ownership must reject visibly.
- Cache persistence uses the [storage atomic-write contract](../../crates/lunco-storage/README.md#usage). Cover concurrent replacement at that generic owner before relying on detached cache writes.
- For lifecycle work, cover old-mount rejection, same-source reopening, exact
  connection replacement, deferred spawn/replay retirement, asynchronous result
  cancellation, and prediction-buffer teardown at their generic owners. The
  scene-free `net_smoke` harness covers Application traffic; it does not prove
  Twin reload behavior. Use an owned host/client session for that acceptance.

## Production deployment

Follow the deployment guide's exact binary, asset, cache, service-account,
firewall, TLS, and nginx layout. Use a real non-development netcode key before
binding a public interface. Verify service logs, certificate renewal, client
connection, and the authenticated API path. A successful local compile is not
deployment evidence.

Networking policy belongs in the networking/deployment owners and authored
scenario policy remains in Rhai. Do not add a second transport, silently open a
port, or claim multiplayer support from a single-process smoke test.
