# Sandbox Operations & Deployment Guide

Build, deploy, and run the LunCoSim Sandbox — the native headless server and the
browser-based WASM client. Deployment is **copy-to-a-path** only: no fixed
install prefix, no systemd service management, no restarts. You pick the paths.

---

## 1. Build

Build both components locally from the repo root before deploying.

### A. Native headless server
```bash
./scripts/build.sh sandbox-server --release
```
*Output → `dist/server/` (binary `sandbox` + `assets/`).*

### B. Web WASM client
```bash
./scripts/build_web.sh build sandbox --release
```
*Output → `dist/sandbox/`.*

---

## 2. Deploy

Just rsync the bundles to paths you give. No default path, no restart.

### Both at once
```bash
./scripts/deploy_server.sh deploy@host \
    --server ~/sandbox-server \
    --web    /var/www/html/sandbox.lunco.space
```
Server-only: drop `--web`. `--dry-run` to preview, `--ssh-port N` for a non-default port.

### Separately (lunica-style wrappers)
```bash
./scripts/deploy_sandbox_server.sh deploy@host:~/sandbox-server                  # binary + assets
./scripts/deploy_sandbox_web.sh    deploy@host:/var/www/html/sandbox.lunco.space  # wasm (brotli/gzip pre-compressed)
```
A target path is required (in the target or as a trailing arg) — the script fails without one.

---

## 3. Run the server

Copying the binary does not start it. Run it however you want on the box — by
hand, tmux/nohup, or whatever process manager you already use:
```bash
cd ~/sandbox-server
./sandbox --host 5888 --api 4101 --cert /etc/letsencrypt/live/sandbox.lunco.space
```
- `--host 5888` — WebTransport host (UDP). `5888` is the default; omit the number to use it.
- `--api 4101` — HTTP admin API on `127.0.0.1`. `4101` is the default; omit the number to use it.

### TLS certificate

The server picks up a CA cert (Let's Encrypt) one of two ways — **point it at the
cert, it does the rest**:

```bash
# Easiest — point at the certbot live dir; it finds fullchain.pem + privkey.pem:
--cert /etc/letsencrypt/live/<domain>

# Or an explicit cert file (+ --key, else the sibling privkey.pem is assumed):
--cert /path/cert.pem --key /path/key.pem
```
Equivalent env vars (CLI wins if both given): `LUNCO_TLS_CERT` + `LUNCO_TLS_KEY`.

Behaviour:
- **Cert specified + valid** → serves it. Logs `🔐 WebTransport using cert from …`.
- **Cert specified + unreadable/invalid** → **panics on boot** (fail-loud; it
  won't silently fall back to a cert browsers reject). Usually a permissions
  issue — `privkey.pem` is `root:root 0600`; run as root or copy the PEMs to a
  path your user can read.
- **Nothing specified** → dev **self-signed** cert. Browsers reject it without a
  pinned digest, but a **native client over a bare IP works with no cert at all**
  (see §4).

Re-reads the cert only at startup — restart the process after a renewal.

---

## 4. Client connection

### Browser
1. Open `https://<your-web-host>/` in a WebGPU-capable browser.
2. **Network → Connect** (pre-fills the page origin `:5888`), or deep-link:
   `https://<your-web-host>/?connect=<server-host>`
3. The browser standalone sandbox also runs fully offline (WASM does the sim) —
   the server is only needed for networked/multiplayer.

### Native
```bash
# Hostname + CA cert (production): validated via the system root store.
./target/release/sandbox --connect sandbox.lunco.space

# Bare IP (LAN/dev): no cert needed — TLS validation is skipped.
./target/release/sandbox --connect 192.168.1.50
```
- **Hostname** → full CA validation (use this for anything public).
- **Bare IP** → validation skipped, so a self-signed server just works. This is
  **insecure (MITM-able)** and meant for LAN/dev only; the client logs a warning
  on each connect.

---

## 5. Troubleshooting

| Symptom | Probable cause | Resolution |
|---|---|---|
| Connection hangs on WebTransport / Network Error | UDP `5888` blocked | Open UDP `5888` (and TCP `80/443` for the web client) in the firewall/security-group. |
| Blank / dark canvas | WebGPU unsupported or a plugin failed | Check browser console; confirm WebGPU support and compatible hardware. |
| Native client cert error on a **hostname** | Hostname doesn't match the cert (or no CA cert) | Use a hostname the cert covers, or dial the bare **IP** (`--connect <ip>`) to skip validation for LAN/dev. |
