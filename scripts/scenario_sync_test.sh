#!/usr/bin/env bash
#
# Two-process scenario-sync integration test (headless, real WebTransport).
#
# Boots a networking HOST serving a twin and a COLD-cache CLIENT that joins over
# the production lightyear wire, then asserts the twin arrived and loaded:
#
#   host:   builds the scenario manifest, serves bytes over HTTP (:5889)
#   client: receives manifest → fetches+verifies bytes → caches → loads
#           `scenario://<id>/<scene>`
#
# Assertions (both automation surfaces exercised where each actually works):
#   - HOST  → rhai (`scripts/scenario_sync_assert.rhai` via `rhai_eval.py`): the
#     prims in $SYNC_HOST_PRIMS exist in the host's scene graph. Exercises the
#     rhai path. (The host sim is resumed first so FixedUpdate — and thus
#     `drain_world_scripts` — ticks; a `--scene`-loaded scene starts paused.)
#   - CLIENT → the sync PIPELINE, twin-agnostically: manifest received, every
#     asset cached + entry-scene load triggered, the on-disk cache populated, no
#     panic, no CID-verify failure. rhai can't run on a client
#     (`scripts_run_here` is false for a client), so this uses logs + the cache.
#   - OPTIONAL client prim check: if $SYNC_CLIENT_PRIMS is set, assert those prims
#     appear on the client via the synchronous `ListEntities` API. (Skipped by
#     default: the in-repo sandbox scene IS the client's baked default, so loading
#     it as a scenario collides and doesn't re-register prims by path — the
#     pipeline check covers it. A DISTINCT twin like moonbase does register them.)
#   - OPTIONAL DEM check: if $SYNC_EXPECT_DEM=1, assert the client built its DEM
#     terrain (guards the native scenario:// terrain-resolution fix).
#
# Both peers use a DISTINCT `LUNCO_PEER_ID` — the exact config that used to crash
# the host on journal replay (fixed: the host bases replay on its own manifest's
# journal_head), so a green run also guards that regression.
#
# Defaults are CI-ready: the in-repo `assets/scenes/sandbox/sandbox_scene.usda`
# twin, host-prim + pipeline assertions, no external assets.
#
# Local moonbase run (full prim + DEM coverage):
#   SYNC_HOST_PRIMS="/MoonbaseScene/Terrain /MoonbaseScene/Avatar" \
#   SYNC_CLIENT_PRIMS="/MoonbaseScene/Terrain /MoonbaseScene/Skid_Physical_1" \
#   SYNC_EXPECT_DEM=1 \
#   scripts/scenario_sync_test.sh ~/Documents/lunco/moonbase/twin/moonbase_scene.usda
#
# Exits 0 on PASS, nonzero on FAIL — CI-gateable, like scripts/net_smoke.sh.
set -uo pipefail
cd "$(dirname "$0")/.."

TWIN="${1:-$PWD/assets/scenes/sandbox/sandbox_scene.usda}"
# Prims the HOST must have (rhai `find`). Default suits the sandbox scene.
SYNC_HOST_PRIMS="${SYNC_HOST_PRIMS:-/SandboxScene/Skid_Physical_1 /SandboxScene/Ackermann_Physical_1}"
# Prims to additionally assert on the CLIENT (ListEntities). Empty = pipeline only.
SYNC_CLIENT_PRIMS="${SYNC_CLIENT_PRIMS:-}"
SYNC_EXPECT_DEM="${SYNC_EXPECT_DEM:-0}"

HOST_API=4101
CLIENT_API=4102
HOST_LOG=/tmp/scenario_sync_host.log
CLIENT_LOG=/tmp/scenario_sync_client.log
BIN=target/debug/sandbox

if [ ! -f "$TWIN" ]; then
  echo "twin scene not found: $TWIN" >&2
  echo "pass a twin's <scene>.usda as \$1 (default: in-repo sandbox scene)" >&2
  exit 2
fi

echo "==> building sandbox (--features networking)"
cargo build --bin sandbox --features networking -j"${SYNC_JOBS:-6}" || exit 2

# Cold cache: force a full download so the HTTP bytes plane is exercised (a warm
# cache fetches nothing). Point BOTH the harness and the launched apps at the SAME
# cache root — the binary, run directly rather than via `cargo`, does not inherit
# the `[env]` in `.cargo/config.toml`. Wipe ONLY `scenarios/`: nuking the whole
# root strips fonts/models the app needs (the "cold-cache isolation gotcha").
CACHE_ROOT="${LUNCOSIM_CACHE:-}"
if [ -z "$CACHE_ROOT" ]; then
  CACHE_ROOT="$(sed -n 's/.*LUNCOSIM_CACHE *= *{ *value *= *"\([^"]*\)".*/\1/p' \
    .cargo/config.toml 2>/dev/null | head -1)"
fi
CACHE_ROOT="${CACHE_ROOT:-$PWD/.cache}"
export LUNCOSIM_CACHE="$CACHE_ROOT"
echo "==> cache root: $CACHE_ROOT (wiping scenarios/ for a cold sync)"
rm -rf "$CACHE_ROOT/scenarios" 2>/dev/null || true

pass=1
fail() { echo "  FAIL: $*"; pass=0; }

HOST_PID=""
CLIENT_PID=""
cleanup() { kill "$HOST_PID" "$CLIENT_PID" 2>/dev/null; true; }
trap cleanup EXIT

rm -f "$HOST_LOG" "$CLIENT_LOG"

echo "==> launching HOST on :5888, api :$HOST_API  (twin: $TWIN)"
env RUST_LOG='info,wgpu=error,naga=warn' LUNCO_PEER_ID=itest-host \
  "$BIN" --host --no-ui --api "$HOST_API" --scene "$TWIN" \
  >"$HOST_LOG" 2>&1 </dev/null &
HOST_PID=$!

for _ in $(seq 1 60); do
  sleep 1
  grep -q "scenario manifest built" "$HOST_LOG" && break
  kill -0 "$HOST_PID" 2>/dev/null || break
done
if ! grep -q "scenario manifest built" "$HOST_LOG"; then
  fail "host never built the scenario manifest"; echo "RESULT: FAIL"; exit 1
fi
echo "    host manifest: $(grep -o 'manifest built: [0-9]* assets' "$HOST_LOG" | head -1)"

echo "==> launching CLIENT (cold cache) -> 127.0.0.1:5888, api :$CLIENT_API"
env RUST_LOG='info,wgpu=error,naga=warn' LUNCO_PEER_ID=itest-client \
  "$BIN" --connect 127.0.0.1 --no-ui --api "$CLIENT_API" \
  >"$CLIENT_LOG" 2>&1 </dev/null &
CLIENT_PID=$!

echo "==> waiting for the client to receive + cache + load the scenario"
cached=0
for _ in $(seq 1 90); do
  sleep 2
  if grep -q "scenario fully cached" "$CLIENT_LOG"; then cached=1; break; fi
  grep -q "panicked at" "$CLIENT_LOG" && break
  kill -0 "$CLIENT_PID" 2>/dev/null || break
done
grep -q "scenario manifest received" "$CLIENT_LOG" || fail "client never received the manifest"
[ "$cached" = 1 ] || fail "client never reported 'scenario fully cached; loading entry scene'"

# --- HOST assertion via rhai -------------------------------------------------
# Resume the host sim first (a `--scene`-loaded scene starts paused: no-autostart
# + DEM hold), so `drain_world_scripts` (FixedUpdate) runs the queued snippet.
echo "==> resuming host sim, then asserting host scene graph via rhai"
python3 - "$HOST_API" <<'PY'
import json, sys, urllib.request
body = json.dumps({"command": "ControlAnimation", "params": {"playing": True}}).encode()
req = urllib.request.Request(f"http://127.0.0.1:{sys.argv[1]}/api/commands", data=body,
                            headers={"Content-Type": "application/json"})
urllib.request.urlopen(req, timeout=5).read()
PY
# Build the injected `ASSERT_PRIMS` rhai array from $SYNC_HOST_PRIMS.
primlist=""
for p in $SYNC_HOST_PRIMS; do primlist="${primlist}\"$p\","; done
prelude="let ASSERT_PRIMS = [$primlist];"
verdict=""
for _ in $(seq 1 20); do
  sleep 1
  verdict="$(python3 scripts/api/rhai_eval.py "$HOST_API" -e "$prelude" -f scripts/scenario_sync_assert.rhai 2>/dev/null | tail -1)"
  [ "$verdict" = "SYNC_OK" ] && break
done
echo "    host rhai verdict: ${verdict:-<none>}"
[ "$verdict" = "SYNC_OK" ] || fail "host rhai scene-graph assertion did not reach SYNC_OK"

# --- CLIENT sync-pipeline assertion (twin-agnostic) --------------------------
# The bytes actually landed in the scenario cache (proves the HTTP plane, not just
# a manifest exchange): the per-scenario cache dir exists with files + an index.
echo "==> asserting the client cached the scenario bytes on disk"
scen_dir="$(find "$CACHE_ROOT/scenarios" -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1)"
if [ -n "$scen_dir" ]; then
  nfiles="$(find "$scen_dir" -type f 2>/dev/null | wc -l | tr -d ' ')"
  echo "    cached: $(du -sh "$scen_dir" 2>/dev/null | cut -f1) in $(basename "$scen_dir") ($nfiles files)"
  [ "$nfiles" -gt 0 ] || fail "scenario cache dir has no files"
  [ -f "$CACHE_ROOT/scenarios/index.json" ] || fail "cached-twins index.json missing"
else
  fail "no per-scenario cache dir under $CACHE_ROOT/scenarios"
fi

# --- OPTIONAL client prim check (distinct twins like moonbase) ---------------
if [ -n "$SYNC_CLIENT_PRIMS" ]; then
  echo "==> asserting CLIENT scene graph via ListEntities"
  client_ok=0
  for _ in $(seq 1 20); do
    sleep 1
    if python3 scripts/api/entities_present.py "$CLIENT_API" $SYNC_CLIENT_PRIMS \
        >/tmp/scenario_sync_prims.txt 2>&1; then
      client_ok=1; break
    fi
  done
  sed 's/^/    /' /tmp/scenario_sync_prims.txt
  [ "$client_ok" = 1 ] || fail "client scene is missing synced prims"
fi

# --- OPTIONAL DEM terrain check (guards the scenario:// terrain fix) ----------
if [ "$SYNC_EXPECT_DEM" = 1 ]; then
  grep -q "\[dem-terrain\] built" "$CLIENT_LOG" || fail "client terrain DEM never built"
  grep -q "dem-terrain\] build failed" "$CLIENT_LOG" && fail "client terrain DEM build FAILED"
fi

# --- Invariants that must hold for any twin ----------------------------------
grep -q "panicked at" "$CLIENT_LOG" && fail "client panicked (pause≠speed-0 regression?)"
grep -q "panicked at" "$HOST_LOG" && fail "host panicked (journal double-apply regression?)"
grep -q "failed CID verification" "$CLIENT_LOG" && fail "client hit a CID-verify failure"

echo "----- host -----"
grep -E "manifest built|serving scenario assets|panicked" "$HOST_LOG" | head -4
echo "----- client -----"
grep -E "manifest received|scenario fully cached|dem-terrain\] built|panicked|failed CID" "$CLIENT_LOG" | head -6

if [ "$pass" = 1 ]; then
  echo "RESULT: PASS"
  exit 0
fi
echo "RESULT: FAIL (host: $HOST_LOG  client: $CLIENT_LOG)"
exit 1
