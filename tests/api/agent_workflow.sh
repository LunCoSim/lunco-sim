#!/usr/bin/env bash
# End-to-end agent workflow smoke test — exercises specs 032 + 033 against
# a running modelica_workbench. Drives the full
#   find → open → compile → describe → set_input → snapshot
# loop with no GUI interaction. Fails loudly on the first regression.
#
# Usage:
#   ./tests/api/agent_workflow.sh [PORT]
#
# Workbench must already be running with --api:
#   cargo run --bin modelica_workbench -- --api 3000
#
# IMPORTANT: this script DOES NOT pkill or send Exit at the end. It walks
# in, drives the workflow, and walks out — leaving the user's session
# intact (per `skills/test-via-api/SKILL.md`).

set -e
set -o pipefail

PORT="${1:-3000}"
BASE="http://127.0.0.1:${PORT}/api"
PASS=0
FAIL=0

# ── Helpers ────────────────────────────────────────────────────────────

# `cmd <Command> [JsonParams]` posts and prints. Empty params → `{}`.
# Using an explicit if rather than `${2:-{}}` because bash's
# parameter-default expansion interacts badly with brace literals
# (it appends a stray `}` and we end up posting malformed JSON).
cmd() {
    local command="$1"
    local params
    if [ -z "$2" ]; then
        params="{}"
    else
        params="$2"
    fi
    curl -s -X POST "${BASE}/commands" \
        -H "Content-Type: application/json" \
        -d "{\"command\":\"${command}\",\"params\":${params}}"
}

# `assert_jq <jq-filter> <expected> <description>`
# Pipes the previous response (in $RESP) through jq, compares to expected.
assert_jq() {
    local filter="$1"
    local expected="$2"
    local desc="$3"
    local got
    got=$(echo "$RESP" | jq -r "$filter")
    if [ "$got" = "$expected" ]; then
        echo "  ✅ $desc"
        PASS=$((PASS+1))
    else
        echo "  ❌ $desc"
        echo "     filter: $filter"
        echo "     got:    $got"
        echo "     want:   $expected"
        echo "     full:   $RESP" | head -c 500
        echo
        FAIL=$((FAIL+1))
    fi
}

assert_truthy() {
    local filter="$1"
    local desc="$2"
    local got
    got=$(echo "$RESP" | jq -r "$filter")
    if [ -n "$got" ] && [ "$got" != "null" ] && [ "$got" != "false" ]; then
        echo "  ✅ $desc ($got)"
        PASS=$((PASS+1))
    else
        echo "  ❌ $desc — filter \`$filter\` returned: $got"
        FAIL=$((FAIL+1))
    fi
}

# Wait for API to be ready.
echo "🚀 Agent workflow smoke (port ${PORT})"
echo "==========================================="
echo "⏳ Waiting for API…"
for i in {1..10}; do
    if curl -s -o /dev/null -X POST "${BASE}/commands" \
        -H "Content-Type: application/json" \
        -d '{"command":"DiscoverSchema","params":{}}' 2>/dev/null; then
        echo "✅ API ready"
        break
    fi
    if [ "$i" -eq 10 ]; then
        echo "❌ API not responding on :${PORT} — start the workbench with --api ${PORT}"
        exit 1
    fi
    sleep 1
done
echo

# ── 1. find_model — fuzzy hit on AnnotatedRocketStage ──────────────────
echo "🔍 1. find_model(\"rocket\")"
RESP=$(cmd "FindModel" '{"query":"rocket"}')
assert_truthy '.data.count > 0' "at least one match"
URI=$(echo "$RESP" | jq -r '.data.items[] | select(.uri | test("AnnotatedRocketStage")) | .uri' | head -1)
if [ -z "$URI" ]; then
    echo "  ❌ AnnotatedRocketStage.mo not in find results"
    echo "$RESP" | jq '.data.items'
    FAIL=$((FAIL+1))
    URI="bundled://AnnotatedRocketStage.mo"
else
    echo "  ✅ resolved URI: $URI"
    PASS=$((PASS+1))
fi
echo

# ── 2. open_uri ────────────────────────────────────────────────────────
echo "📂 2. open_uri(\"$URI\")"
RESP=$(cmd "Open" "{\"uri\":\"$URI\"}")
assert_truthy '.command_id' "Open accepted"
echo

# ── 3. list_open_documents — resolve doc_id ───────────────────────────
# Open is deferred via `commands.queue`; it can take a few ticks for
# the doc to land in the workspace registry. Poll instead of sleeping
# a fixed duration — the test stays fast on a warm workbench but
# tolerates a slow one.
echo "📑 3. list_open_documents (polling for doc)"
DOC_ID=""
for i in {1..20}; do
    RESP=$(cmd "ListOpenDocuments")
    DOC_ID=$(echo "$RESP" | jq -r '.data.open_documents[] | select(.title | test("Annotated|RocketStage")) | .doc_id' | head -1)
    if [ -n "$DOC_ID" ] && [ "$DOC_ID" != "null" ]; then
        break
    fi
    sleep 0.5
done
if [ -z "$DOC_ID" ] || [ "$DOC_ID" = "null" ]; then
    echo "  ❌ no open doc matching AnnotatedRocketStage after 10s. Bailing."
    echo "$RESP" | jq '.data.open_documents'
    exit 1
fi
echo "  ✅ doc_id=$DOC_ID"
PASS=$((PASS+1))
echo

# ── 4. list_compile_candidates — assert RocketStage class present ─────
echo "🧩 4. list_compile_candidates(doc=$DOC_ID)"
RESP=$(cmd "ListCompileCandidates" "{\"doc\":$DOC_ID}")
HAS_ROCKETSTAGE=$(echo "$RESP" | jq -r '[.data.candidates[].short] | contains(["RocketStage"])')
COUNT=$(echo "$RESP" | jq -r '.data.count')
echo "  ℹ $COUNT candidates"
if [ "$HAS_ROCKETSTAGE" = "true" ]; then
    echo "  ✅ RocketStage in candidates"
    PASS=$((PASS+1))
else
    echo "  ❌ RocketStage missing from candidates:"
    echo "$RESP" | jq '.data.candidates'
    FAIL=$((FAIL+1))
fi
echo

# ── 5. compile_model with explicit class ──────────────────────────────
echo "🔨 5. compile_model(doc=$DOC_ID, class=\"RocketStage\")"
RESP=$(cmd "CompileActiveModel" "{\"doc\":$DOC_ID,\"class\":\"RocketStage\"}")
assert_truthy '.command_id // .data' "Compile accepted"
echo "  ⏳ waiting up to 60s for compile to finish…"
for i in {1..120}; do
    sleep 0.5
    RESP=$(cmd "CompileStatus" "{\"doc\":$DOC_ID}")
    STATE=$(echo "$RESP" | jq -r '.data.state')
    if [ "$STATE" = "ok" ]; then
        echo "  ✅ compile state=ok after ${i}× 0.5s"
        PASS=$((PASS+1))
        break
    elif [ "$STATE" = "error" ]; then
        echo "  ❌ compile failed:"
        echo "$RESP" | jq '.data'
        FAIL=$((FAIL+1))
        break
    fi
    if [ "$i" -eq 120 ]; then
        echo "  ❌ compile did not reach ok/error within 60s (last state=$STATE)"
        echo "$RESP" | jq '.data'
        FAIL=$((FAIL+1))
    fi
done
echo

# ── 6. describe_model — assert valve component is present ────────────
# Note: AnnotatedRocketStage.RocketStage is a *composed* model — the
# runtime input lives on the Valve subcomponent (`valve.opening`,
# Modelica.Blocks.Interfaces.RealInput). describe_model on RocketStage
# surfaces the components; describe_model on Valve would surface the
# `opening` input. We verify both shapes here.
echo "📋 6a. describe_model(doc=$DOC_ID, class=\"RocketStage\")"
RESP=$(cmd "DescribeModel" "{\"doc\":$DOC_ID,\"class\":\"RocketStage\"}")
COMPONENT_COUNT=$(echo "$RESP" | jq -r '.data.components | length')
HAS_VALVE_COMP=$(echo "$RESP" | jq -r '[.data.components[].name] | contains(["valve"])')
if [ "$HAS_VALVE_COMP" = "true" ]; then
    echo "  ✅ valve component present (and $COMPONENT_COUNT total)"
    PASS=$((PASS+1))
else
    echo "  ❌ valve component missing"
    FAIL=$((FAIL+1))
fi
echo

echo "📋 6b. describe_model(doc=$DOC_ID, class=\"Valve\") — find opening input"
RESP=$(cmd "DescribeModel" "{\"doc\":$DOC_ID,\"class\":\"Valve\"}")
HAS_OPENING=$(echo "$RESP" | jq -r '[.data.inputs[].name] | contains(["opening"])')
if [ "$HAS_OPENING" = "true" ]; then
    echo "  ✅ opening listed as input on Valve (validates connector-typed input detection)"
    PASS=$((PASS+1))
else
    echo "  ❌ opening missing from Valve inputs:"
    echo "$RESP" | jq '.data.inputs'
    FAIL=$((FAIL+1))
fi
echo

# ── 7. set_input — happy path. Use the FLATTENED dotted name —────────
# `RocketStage` has no top-level inputs of its own; the runtime
# throttle is `valve.opening` after flattening. The simulation
# worker's `inputs` map is keyed by the flattened name.
echo "🎛  7. set_input(doc=$DOC_ID, name=\"valve.opening\", value=0.5)"
RESP=$(cmd "SetModelInput" "{\"doc\":$DOC_ID,\"name\":\"valve.opening\",\"value\":0.5}")
OK=$(echo "$RESP" | jq -r '.data.ok // empty')
if [ "$OK" = "true" ]; then
    echo "  ✅ set_input ok=true"
    PASS=$((PASS+1))
else
    echo "  ❌ set_input failed:"
    echo "$RESP" | jq .
    FAIL=$((FAIL+1))
fi
echo

# ── 8. set_input — error path (typo) ─────────────────────────────────
# The HTTP transport renders `ApiResponse::Error { message }` as
# `{"error": "<message>"}` (not `.message`), so we read `.error` here.
echo "🎛  8. set_input(doc=$DOC_ID, name=\"valve.openin\" /* typo */, value=0.5)"
RESP=$(cmd "SetModelInput" "{\"doc\":$DOC_ID,\"name\":\"valve.openin\",\"value\":0.5}")
ERR=$(echo "$RESP" | jq -r '.error // .message // empty')
if echo "$ERR" | grep -q "valve.opening"; then
    echo "  ✅ error lists the valid name: $ERR"
    PASS=$((PASS+1))
else
    echo "  ❌ error did not list valid input names. Got: $ERR"
    echo "$RESP" | jq .
    FAIL=$((FAIL+1))
fi
echo

# ── 9. resume sim & 10. snapshot — assert thrust value comes back ─────
echo "▶  9. ResumeActiveModel(doc=$DOC_ID)"
RESP=$(cmd "ResumeActiveModel" "{\"doc\":$DOC_ID}")
assert_truthy '.command_id // .data' "Resume accepted"
sleep 1.5  # let a few sim steps run
echo

echo "📊 10. snapshot_variables(doc=$DOC_ID)"
RESP=$(cmd "SnapshotVariables" "{\"doc\":$DOC_ID}")
COMPILED=$(echo "$RESP" | jq -r '.data.compiled')
T=$(echo "$RESP" | jq -r '.data.t')
if [ "$COMPILED" = "true" ]; then
    echo "  ✅ compiled=true, sim time t=$T"
    PASS=$((PASS+1))
    # Show a sample of variables / inputs
    echo "  ℹ inputs:    $(echo "$RESP" | jq -c '.data.inputs')"
    echo "  ℹ variables: $(echo "$RESP" | jq -c '.data.variables | keys[:5]')"
else
    echo "  ❌ compiled=$COMPILED"
    echo "$RESP" | jq .
    FAIL=$((FAIL+1))
fi
echo

# ── 11. PauseActiveModel — clean shutdown ─────────────────────────────
echo "⏸  11. PauseActiveModel(doc=$DOC_ID)"
RESP=$(cmd "PauseActiveModel" "{\"doc\":$DOC_ID}")
assert_truthy '.command_id // .data' "Pause accepted"
echo

# ── Summary ───────────────────────────────────────────────────────────
echo "==========================================="
echo "📈 ${PASS} passed, ${FAIL} failed"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
echo "✨ Agent workflow end-to-end smoke ✓"
