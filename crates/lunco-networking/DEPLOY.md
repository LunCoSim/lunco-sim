# Deploying `sandbox.lunco.space` (headless server + wasm client)

This kit deploys two things on one Ubuntu box:

| Component | What | Port | Process |
|---|---|---|---|
| **Sim host** | `sandbox --no-ui --host` — server-authoritative sim + WebTransport host | **UDP 5888** | `lunco-server.service` |
| **Web client** | the wasm bundle browsers load | **TCP 443** | nginx (static files) |
| Admin API | HTTP command API (loopback only) | TCP 4101 (`127.0.0.1`) | inside the sim host |

Both are served against **one** Let's Encrypt cert for `sandbox.lunco.space`.
Browsers validate the WebTransport host via the normal CA chain — **no cert
digest / URL `#hash`** is needed in production (that dance is localhost-only).

WebTransport is HTTP/3 = QUIC = **UDP**. nginx cannot reverse-proxy it, so the
sim host owns UDP 5888 directly and reads the same cert files.

---

## Quick path (scripts)

The build + deploy is scripted; the manual sections below are what those scripts
do under the hood.

```bash
# Build both artifacts (server binary + wasm client):
./scripts/build.sh sandbox-server --release
./scripts/build_web.sh build sandbox --release

# Redeploy (certs already managed by Let's Encrypt — the common case):
# 1. Deploy native server (binary + assets):
./scripts/deploy_sandbox_server.sh deploy@sandbox.lunco.space
# 2. Deploy web client (WASM):
./scripts/deploy_sandbox_web.sh deploy@sandbox.lunco.space

# First-time provisioning only (installs apt deps, systemd unit, nginx, certbot):
./scripts/deploy_server.sh deploy@sandbox.lunco.space --provision --email you@lunco.space
```

`deploy_server.sh` is retained for bootstrap/provisioning and supports the flags: `--prefix`, `--ssh-port`, `--no-restart`, `--dry-run`, `--provision` (sub-flags: `--domain`, `--email`, `--self-signed`, `--no-cert`, `--stage`).

---

## 0. Prerequisites (manual reference)

```bash
sudo apt update
# Web tier + tooling:
sudo apt install -y nginx certbot python3-certbot-nginx ufw rsync
# Server binary runtime libs. The headless binary links NO GPU/Vulkan/X11/audio
# (backends:None never loads a graphics driver) — `ldd` shows only these beyond
# libc. `libwayland-client0` is linked by winit but UNUSED (WinitPlugin is
# disabled under --no-ui); it must still be PRESENT or the dynamic loader fails
# at exec. libudev1 ships with systemd. TLS is static rustls — no libssl needed.
sudo apt install -y libwayland-client0 libudev1
# Build host needs the Rust toolchain (rustup) + the repo. The build can run on
# the same box or anywhere x86_64-linux; only the binary + assets get shipped.
```

> **glibc compatibility.** The binary is a glibc-dynamic ELF. If the server's
> Ubuntu is **older** than the build host, it fails at startup with
> `GLIBC_x.xx not found`. Either **build on the server box**, or build in a
> container matching the server's Ubuntu release. (Same glibc-or-newer rule for
> `libwayland-client0`/`libudev1`.) No GPU, Xvfb, or display is needed on the
> server regardless.

DNS: an `A`/`AAAA` record for `sandbox.lunco.space` → this box's public IP.

---

## 1. Build the artifacts

From a checkout of this repo (`networking` branch):

```bash
# (a) headless server binary — native release. Runs headless by default (no winit/egui).
cargo build --release --bin sandbox-server -p lunco-sandbox-server
#   -> target/release/sandbox-server

# (b) wasm client bundle.
./scripts/build_web.sh build sandbox
#   -> dist/sandbox/   (index.html + *_bg.wasm + js + worker + assets)
```

> The native binary loads assets from `<workdir>/assets`, so the `assets/` tree
> must ship next to the binary (step 3).

---

## 2. Service account + layout

```bash
sudo useradd --system --home /opt/lunco --shell /usr/sbin/nologin lunco
sudo install -d -o lunco -g lunco /opt/lunco /opt/lunco/certs /opt/lunco/.cache /opt/lunco/web
```

Target layout on the box:

```
/opt/lunco/
├── sandbox                 # the release binary
├── assets/                 # asset tree (scenes/, shaders/, models cache, …)
├── certs/                  # deploy-hook-copied fullchain.pem + privkey.pem
├── .cache/                 # rumoca/MSL/model cache (service-writable)
├── lunco-server.env        # config (TLS paths, RUST_LOG)
└── web/sandbox/            # the wasm bundle nginx serves
```

---

## 3. Ship the files (from the build host)

```bash
DEST=root@sandbox.lunco.space         # or your ssh alias
rsync -av target/release/sandbox        $DEST:/opt/lunco/sandbox
rsync -av --delete assets/              $DEST:/opt/lunco/assets/
rsync -av --delete dist/sandbox/        $DEST:/opt/lunco/web/sandbox/
rsync -av scripts/deploy/               $DEST:/opt/lunco/deploy/   # unit, env, hook, nginx conf
rsync -av crates/lunco-networking/DEPLOY.md $DEST:/opt/lunco/deploy/DEPLOY.md  # this runbook, on-box

# Fix ownership on the box:
sudo chown -R lunco:lunco /opt/lunco/sandbox /opt/lunco/assets
sudo chmod 0755 /opt/lunco/sandbox
```

---

## 4. Firewall

```bash
sudo ufw allow 22/tcp           # keep your ssh session alive!
sudo ufw allow 80/tcp           # ACME http-01 challenge + http->https redirect
sudo ufw allow 443/tcp          # wasm bundle (HTTPS)
sudo ufw allow 5888/udp         # WebTransport / QUIC  <-- the easy-to-forget one
sudo ufw enable
```

The admin API (4101) is **not** opened — it binds `127.0.0.1` only. For remote
admin, SSH-tunnel it: `ssh -L 4101:127.0.0.1:4101 $DEST`.

---

## 5. TLS cert + auto-renew hook

Install the deploy hook (copies the cert where `lunco` can read it + restarts
the service on every renewal — the server reads the PEM only at startup):

```bash
sudo install -m 0755 /opt/lunco/deploy/certbot-deploy-hook.sh \
     /etc/letsencrypt/renewal-hooks/deploy/lunco-server.sh
```

Issue the cert (nginx must be installed first — step 6 — or use
`certbot certonly --standalone` with nginx stopped). The `--nginx` plugin also
wires the `ssl_*` lines into the vhost:

```bash
sudo certbot --nginx -d sandbox.lunco.space \
     --deploy-hook /etc/letsencrypt/renewal-hooks/deploy/lunco-server.sh
```

Renewal is automatic (certbot's systemd timer); the deploy hook re-copies the
cert and restarts `lunco-server` each time. Verify the renew path end-to-end:

```bash
sudo certbot renew --dry-run
ls -l /opt/lunco/certs/          # fullchain.pem + privkey.pem, owned by lunco
```

> Either RSA or ECDSA Let's Encrypt certs work (browser does normal chain
> validation). ECDSA is a smaller handshake: add `--key-type ecdsa` to certbot.

---

## 6. nginx (serve the wasm bundle)

```bash
sudo cp /opt/lunco/deploy/nginx-sandbox.lunco.space.conf \
        /etc/nginx/sites-available/sandbox.lunco.space
sudo ln -sf /etc/nginx/sites-available/sandbox.lunco.space \
            /etc/nginx/sites-enabled/sandbox.lunco.space
sudo nginx -t && sudo systemctl reload nginx
```

(If you ran certbot before nginx existed, re-run `sudo certbot --nginx ...` now
so it fills in the `ssl_certificate*` lines, or paste them by hand.)

---

## 7. Configure + start the sim host

```bash
sudo cp /opt/lunco/deploy/lunco-server.env /opt/lunco/lunco-server.env   # edit if needed
sudo chown root:lunco /opt/lunco/lunco-server.env && sudo chmod 0640 /opt/lunco/lunco-server.env

sudo cp /opt/lunco/deploy/lunco-server.service /etc/systemd/system/lunco-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now lunco-server
```

Check it:

```bash
systemctl status lunco-server
journalctl -u lunco-server -f
# Expect:
#   🔐 WebTransport using cert from /opt/lunco/certs/fullchain.pem
#   [net] host listening on 0.0.0.0:5888
#   [net] sandbox running HEADLESS (--no-ui): ...
#   Loading sandbox scene ... via LoadScene
```

If the cert env is set but the PEM is bad, the service **panics on boot by
design** (fail-loud) — `systemctl status` shows it `failed`; the journal line
names the exact path/permission problem. Fix the cert, `systemctl start`.

---

## 8. Verify from a browser

Open `https://sandbox.lunco.space/`, then in the top menu **Network → Connect**
(the address pre-fills the page origin → `sandbox.lunco.space:5888`). The
journal should log `New connection on netcode … sent N-entity state baseline …
client connected`, and the replicated scene appears.

---

## Redeploy (new build)

```bash
# rebuild (step 1), then:
rsync -av target/release/sandbox $DEST:/opt/lunco/sandbox && sudo systemctl restart lunco-server
rsync -av --delete dist/sandbox/ $DEST:/opt/lunco/web/sandbox/   # no restart needed
```

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Service `failed` immediately, journal `🔐 … cert could not be loaded` | PEM path/perms wrong, or only one of `LUNCO_TLS_CERT`/`KEY` set. Fail-loud by design — fix the env/cert. |
| Browser connects to the page but Network → Connect hangs / `WebTransport` error | UDP 5888 not open (`ufw allow 5888/udp`), or a NAT/cloud security-group UDP rule missing. |
| `host listening` but baseline is `0-entity` | scene loaded but no dynamic bodies tagged — check the scene actually spawns rovers/props. |
| Cert renewed but browser still sees the old expiry | deploy hook didn't run/restart — check `/etc/letsencrypt/renewal-hooks/deploy/lunco-server.sh` is executable and `journalctl -u lunco-server` shows a restart at renew time. |
| `wasm` 404 / wrong MIME | nginx `types { application/wasm wasm; }` missing or bundle not under `/opt/lunco/web/sandbox`. |

See also: `src/server.rs` (cert handling),
`../lunco-sandbox/src/bin/sandbox.rs` (`--no-ui` headless wiring).
The deploy config files live in `../../scripts/deploy/`.
