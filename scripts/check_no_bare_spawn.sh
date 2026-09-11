#!/usr/bin/env bash
# Guard against regressions in the StatusBus loading-lifecycle invariant.
#
# Rule: any `AsyncComputeTaskPool::get().spawn(...)` whose output
# eventually surfaces in a UI panel must register a `StatusBus`
# `BusyHandle` (typically via
# `lunco_workbench::tracked_task::spawn_tracked[_cancellable]`).
# Without that, the panel's overlay can flicker through "empty" or
# "missing" while work is still in flight.
#
# This check is a structural reminder, not a perfect static analysis.
# It greps for bare `AsyncComputeTaskPool::get().spawn(` and fails
# unless every match is on the allowlist below — with a comment
# explaining the exception (handle is minted at the binding insert
# site, work doesn't surface in the UI, doctest, etc.).
#
# Usage: scripts/check_no_bare_spawn.sh
# Wire into CI / pre-commit as needed.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Allowlist format: one `path[:line]` per entry, '#' comments allowed.
# Paths are relative to repo root. Prefer whole-file entries (stable
# across line shifts) and group the justifications for every spawn
# in that file under a single comment. Update when a new legitimate
# spawn site is added — and add a comment explaining why it doesn't
# need spawn_tracked.
ALLOWLIST=$(cat <<'EOF'
# Wrapper itself — this is THE primitive every caller goes through.
crates/lunco-workbench/src/tracked_task.rs

# Doc-comment reference (line 65) + doctest fixture (line 202).
crates/lunco-cache/src/lib.rs

# File picker — one-shot modal dialog with its own progress indicator;
# no canvas/panel overlay depends on this completing.
crates/lunco-workbench/src/picker.rs

# DrillIn / Duplicate / FileLoad spawns. The BusyHandle is minted at
# the binding-insert site (DrillInBinding._busy / DuplicateBinding._busy
# / OpeningState::FileLoad._busy) so the bus is busy for the full
# parse stage — the spawn itself does not own the handle.
crates/lunco-modelica-ui/src/ui/commands/lifecycle.rs
crates/lunco-modelica-ui/src/ui/panels/canvas_diagram/loads.rs
crates/lunco-modelica-ui/src/ui/panels/package_browser/mod.rs

# Background image (port icon) decode. Result lands in an asset cache;
# no overlay state depends on the spawn completing within a frame.
# Sweep into spawn_tracked with BusyScope::Global if a future
# UI surface starts to care about decode progress.
crates/lunco-modelica-ui/src/ui/image_loader.rs

# Diagnostics panel background work — populates a sidebar list that
# is itself displayed only when non-empty; no "loading" affordance
# yet to keep honest. Migrate when the panel grows one.
crates/lunco-modelica-ui/src/ui/panels/diagnostics.rs:308

# Doc comment / example, not a real spawn.
crates/lunco-modelica-core/src/engine_resource.rs:98
EOF
)

# Build "path:line" set of allowed sites. Comments + blank lines stripped.
allowed=$(echo "$ALLOWLIST" | grep -vE '^\s*(#|$)' | sed 's/[[:space:]]\+$//')

# Find every match. We track file:line; whole-file allowlist entries
# (no `:line`) match any line in that file.
violations=0
while IFS=: read -r file line _; do
    [[ -z "$file" ]] && continue
    match="$file:$line"
    if echo "$allowed" | grep -qxF "$match"; then
        continue
    fi
    if echo "$allowed" | grep -qxF "$file"; then
        continue
    fi
    if [[ $violations -eq 0 ]]; then
        echo "error: bare AsyncComputeTaskPool::get().spawn(...) outside allowlist."
        echo "       Route the spawn through lunco_workbench::tracked_task::spawn_tracked"
        echo "       (or add the site to scripts/check_no_bare_spawn.sh with a justification)."
        echo
    fi
    violations=$((violations + 1))
    echo "  $match"
done < <(grep -rnF "AsyncComputeTaskPool::get().spawn(" crates/ --include='*.rs' || true)

if [[ $violations -gt 0 ]]; then
    echo
    echo "$violations violation(s)."
    exit 1
fi

echo "ok — every bare pool.spawn is accounted for."
