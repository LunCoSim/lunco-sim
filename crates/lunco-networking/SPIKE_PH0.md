# Ph0 spike — runbook (lightyear)

Goal: de-risk **lightyear on Bevy 0.18 + browser WebTransport + client prediction +
host-client (listen-server)** before committing the backend. Decision gate at the end.

**The spike is just: run lightyear's own example in a throwaway clone.** No crate of
ours, no copying — that answers "does lightyear work for us?" Our own app is written
later, at real integration (Ph1+), as a plain crates.io dependency.

**Verified versions (2026-05-29) — the whole triangle aligns with our workspace:**
- `lightyear 0.26.4` ⇒ **Bevy 0.18** ✅ (workspace `bevy 0.18.1`)
- `avian3d 0.6` ✅ (workspace pins `0.6.1`) — via `lightyear_avian3d 0.26` (Step C/Ph3)
- `bevy_egui 0.39` ✅ (workspace `0.39.1`)
- lightyear's own `simple_box` example **already does prediction + WASM/WebTransport
  + host-client** — we exploit that.

lightyear API surface (noted now, used at real integration in Ph1+):
- features: `interpolation, prediction, replication, input_native` (+ `client`,
  `server`, `netcode`, `udp`; add `webtransport` for browser).
- `app.register_component::<T>().add_prediction().add_linear_interpolation();`
- `app.add_channel::<C>(ChannelSettings { mode: OrderedReliable(..), .. }).add_direction(ServerToClient);`
- inputs via `input_native`: an `Inputs` enum + `register_input`; `ProtocolPlugin`
  registers components/messages/channels/inputs.

Key facts from lightyear:
- `simple_box`: **pink cube = client-predicted** (input applied with no delay,
  rollback on server mismatch), **red cube = received server state**.
- Run host-client (our topology): `cargo run -- host-client -c 0`.
- WASM/WebTransport needs a **valid SSL cert**; **Safari unsupported** — test in
  Chrome/Edge/Firefox.

---

## Step A — Run lightyear's own example  ← this *is* the spike
Proves the whole stack with known-good code. **No crate of ours is needed to make
the decision** — we just run their example in the clone. (Earlier drafts added a
`spike-ph0/` copy-the-example crate; that was needless ceremony — removed. Our own
lightyear app gets written for real in Ph1+ as a normal crates.io dependency, when
we're integrating, not validating.)

```sh
# somewhere OUTSIDE our worktrees (it's a throwaway clone)
git clone --depth 1 --branch 0.26.4 https://github.com/cBournhonesque/lightyear
cd lightyear/examples/simple_box

# 1) native host-client (also the server) — desktop window
cargo run -j 2 -- host-client -c 0

# 2) a native client joining it (second terminal)
cargo run -j 2 -- client -c 1
```

**Success (native):** two windows; moving the box on one updates the other; the
predicted (pink) box responds to *your* input instantly, the server (red) box trails.

### Browser client (the part that actually matters for us)
WebTransport needs a trusted cert. Use mkcert (one-time, both your machine and your
friend's later):
```sh
# one-time
mkcert -install
# cert for localhost + your LAN IP (for the friend later)
mkcert localhost 127.0.0.1 <your-LAN-IP>
```
Then run the server with that cert and build the wasm client per the example's
README (`examples/simple_box/README.md` — it documents the wasm + cert flags;
lightyear examples accept a certificate-hash / cert path for WebTransport).

**Success (browser):** a Chrome/Edge/Firefox tab connects, shows the predicted +
server boxes, and your input moves the predicted box with no delay.

### Stress it (this is the real test)
The example supports a network conditioner (added latency/jitter/loss). Run with
~80–150 ms latency + jitter and confirm:
- predicted box stays responsive (no input lag),
- corrections are **smooth**, not teleporting,
- **host-client** mode behaves (this is lightyear's historically rough spot — the
  whole reason we spike).

---

## Decision gate (Step A is enough to decide)
| Outcome | Decision |
|---|---|
| native + browser smooth, incl. host-client under latency | **Commit lightyear.** Proceed to Ph1 (M6 clock + M1 identity), then write our own lightyear app for real (below). |
| **host-client** broken/janky | Try dedicated-server topology (still lightyear) **or** the `lightyear-fix-host-client-replication` fork; if neither, **fall back to replicon+renet2** (M4/M6 hand-built; our facade keeps domain code unchanged). |
| won't build/connect on Bevy 0.18 | Re-check the git tag; if genuinely broken, **fall back to replicon+renet2**. |

---

## After the decision — integrate for real (Ph1+, NOT a copy of the example)
Once committed, our lightyear app is just part of normal feature work:
- a normal crate in the workspace with `lightyear = "0.26.4"` from **crates.io** (no
  clone, no `lightyear_examples_common`, no copying);
- we write **our own** thin app (~a few hundred lines) using the API surface noted
  above — `ProtocolPlugin` (register `CubePosition`/`DriveRover`), client/server
  plugins, our transports;
- compiling happens anyway at that point, so there's no value in a separate spike
  crate beforehand.

Then wire the Tier-2 headless **crossbeam** integration tests (`NETWORKING_TEST_PLAN.md`)
against it, and bring in avian (crib `avian_3d_character`) at Ph3.

---

## Constraints honored
- Builds are `-j 2`; first lightyear+bevy build is long — run when ready, not backgrounded.
- The clone is a **throwaway outside the worktrees**; nothing of ours depends on it.
- Verification is visual in the app/browser (yours to run); this doc is the exact sequence.

---

## RESULTS — run 2026-05-29 (Claude, headless native + browser)

Clone: `lightyear 0.26.4` (tag verified), Bevy `0.18` workspace, at
`/home/rod/Documents/lightyear-spike` (throwaway, outside worktrees). Built `-j 2`.

### Native build + host-client robustness — **PASS**
Cold build clean, exit 0, 20m56s. Three runs of the built `simple_box`
(`host-client -c 0` + a joining `client -c 1`, logs captured):

| Check | Result |
|---|---|
| Builds on Bevy 0.18 | ✅ clean (retires the "won't build" gate row) |
| Host-client boots + in-app connect | ✅ `Connected host-client entity=141v0` |
| Server WebTransport listen | ✅ `Server WebTransport starting at 0.0.0.0:5888` |
| Remote client netcode/WebTransport handshake | ✅ `Client Netcode(1) connected` |
| Replication (server spawns per-client players) | ✅ |
| Prediction engaged | ✅ `Add InputMarker to Predicted entity` |
| Tick-sync stability, 30 s, normal join | ✅ **zero** rollback warnings |
| Panics / errors, all runs | ✅ **none** |

**One characterized anomaly (not a blocker):** joining a host-client already
~12 s / 449 ticks ahead, with *no input ever sent*, produced a single
`Trying to do a rollback of 252 ticks. The max is 100! Aborting` — lightyear
**capped at 100 and snapped to server state** (no crash). A re-run joining at +3 s
reproduced it **0 times**. Conclusion: late-join tick-sync transient, handled
gracefully; covered by gap I (late-join baseline snapshot). The conditioner
(`LinkConditionerConfig::average_condition()`) is **on by default** for joining
clients, so the connectivity PASS is already under simulated latency.

### Not coverable headless (needs a human at the keyboard)
Cubes only move on keyboard input, which can't be injected headless, so the
*subjective* checks remain manual: (1) predicted box moves with no perceptible lag;
(2) corrections look smooth, not teleporting, while actively driving.

### Browser / wasm (WebTransport) — **PASS** (connect + replicate)
Tooling: trunk 0.21.14, wasm-bindgen 0.2.122, wasm-opt, google-chrome. Build:
```
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
  trunk build --no-default-features --features=client,netcode index.html
```
Served via `trunk serve` on `:8080`; native `host-client -c 0` is the WebTransport
server (UDP/QUIC :5888). Driven through Chrome DevTools (CDP).

| Check | Result |
|---|---|
| wasm builds (cold ~18 min; leaf-only rebuild ~17 s) | ✅ |
| wasm boots + renders (WebGL via ANGLE) | ✅ |
| **Connects over WebTransport** | ✅ `New connection on netcode …` (server-side) |
| Server creates + **replicates** a player entity to the browser | ✅ (observed twice) |
| Reconnect after a drop | ✅ clean re-handshake |
| No client/sync errors in console | ✅ (only a benign `integrity` preload warn) |
| Stays connected under headless CDP control | ⚠️ no — Chrome throttles the backgrounded tab → WebTransport keepalive lapses → server times the client out. Benign; a foregrounded tab stayed up ~2.4 min. |

**Not visually captured:** the in-browser *interactive* input→prediction→correction
feel — CDP-driving backgrounds the tab and the throttled connection drops before
movement can be screenshotted. That path is already proven on native; to eyeball it,
focus the Chrome window as the active OS window and drive the arrows by hand.

### ⚠️ Dev-cert gotchas for browser WebTransport (cost us real time — read before Ph-browser)
1. **WebTransport mandates TLS** (HTTP/3/QUIC). There is no plaintext mode, even on
   localhost. `--ignore-certificate-errors` does **not** cover QUIC — dead end.
2. **`mkcert`/CA trust does NOT help this example.** The wasm always sends a non-empty
   `serverCertificateHashes`, which forces Chrome's **hash-only** path and *ignores*
   CA validation. A CA-trusted (long-lived) cert is rejected by the hash-path
   constraints. The intended dev path here is the hash, not a CA.
3. **`serverCertificateHashes` constraints** (all required): ECDSA **P-256**, X.509
   **v3**, validity **< 14 days**, and the SHA-256 of the DER cert must equal the
   hash the client sends. `certificates/generate.sh` uses `-days 14` (boundary) and
   ships **stale/expired** sample certs — regenerate fresh (we used `-days 13`).
4. **The digest is baked into the wasm at *compile time*** via
   `include_str!("…/digest.txt")`. The example's `get_digest_on_wasm()` (URL-hash
   override) is **dead code — never called**, so the URL `#hash` is ignored. If the
   server cert changes, you must **rebuild the wasm** (fast, leaf-only ~17 s) so the
   baked digest matches, OR wire the override (needs `web-sys`, which isn't a client
   dep — non-trivial). This stale-baked-digest mismatch was the root cause of every
   browser handshake failure.
5. **Background-tab throttling** drops WebTransport; keep the tab foregrounded for
   interactive testing.

For **our own** Ph-browser client we should wire the URL-hash digest (or a CA path
for non-WebTransport transports) so cert rotation doesn't force wasm rebuilds.

**Gate status:** D1 risk (host-client robustness) **RETIRED** natively; browser
WebTransport **connect + replicate PASS**. Only the subjective in-browser input-feel
remains as a manual eyeball — non-gating.
