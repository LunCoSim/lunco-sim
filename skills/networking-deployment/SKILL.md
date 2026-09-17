---
name: networking-deployment
description: Configure, smoke-test, or deploy LunCoSim networking, the headless server, WebTransport, multiplayer sessions, TLS, or host/client launch. Use for remote simulation, networking feature builds, firewall, service, and deployment questions.
---

# Networking and deployment

Read [`crates/lunco-networking/DEPLOY.md`](../../crates/lunco-networking/DEPLOY.md),
the networking [`README.md`](../../crates/lunco-networking/README.md), and the
relevant synchronization contract before editing. `lunco-networking` owns
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

## Production deployment

Follow the deployment guide's exact binary, asset, cache, service-account,
firewall, TLS, and nginx layout. Use a real non-development netcode key before
binding a public interface. Verify service logs, certificate renewal, client
connection, and the authenticated API path. A successful local compile is not
deployment evidence.

Networking policy belongs in the networking/deployment owners and authored
scenario policy remains in Rhai. Do not add a second transport, silently open a
port, or claim multiplayer support from a single-process smoke test.
