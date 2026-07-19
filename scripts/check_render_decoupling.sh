#!/bin/bash
# ============================================================================
# LunCoSim — render-decoupling gate
# ============================================================================
# Asserts the contract in docs/architecture/render-decoupling.md: the
# `--no-ui` server links no GPU stack. A domain crate may name `Mesh3d`; it
# may not name `MeshMaterial3d`, because the material is what drags
# bevy_pbr → bevy_render → wgpu/naga into the whole graph.
#
# Why this needs a machine, not vigilance (the doc's own section title):
# cargo unifies features across the entire graph, so ONE missing
# `default-features = false` anywhere silently re-links wgpu into every
# binary. It has already happened twice in this repo, and the last edge
# before that was a single billboard `Text2d` label on a spacecraft —
# `bevy_sprite_render` pulls `bevy_render`. Nobody would guess the server
# links a GPU driver because of a text label. Only `cargo tree` sees it.
#
# The docs have referenced this gate since 2026-07-13 ("The render-decoupling
# CI job enforces all of the above. Do not delete it") while no such job
# existed. This is that gate.
#
# NOTE: `cargo tree -i <crate>` reports absence as "warning: nothing to
# print." on stdout with exit code 0 — NOT a non-zero exit and not the
# "package ID not found" string the docs quote. Grepping for the wrong
# sentinel silently inverts every assertion here, which is exactly the
# failure this script exists to prevent, so the check keys on that string.
#
# `-e normal` is REQUIRED. Without it `cargo tree -i` also walks dev- and
# build-dependencies, where wgpu legitimately appears (visual examples,
# `lunco-usd`'s dev-only bevy_pbr), reporting a regression that is not one.
#
# Usage:
#   scripts/check_render_decoupling.sh
#
# Exit codes:
#   0  — the server links no GPU stack
#   1  — a GPU crate is back in the headless graph; the offending chain is printed
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

# The headless server. Anything reachable from here on normal edges ships in
# the `--no-ui` binary and in the wasm worker.
PKG="lunco-sandbox-server"

# `naga` is deliberately ABSENT from this list. It remains in the graph via
# `bevy_shader`, which supplies the WGSL compiler that `SetShaderSource` /
# `CreateShader` use to compile shader edits into `Assets<Shader>` without a
# disk round-trip. A compiler is not a GPU stack. Moving it behind the gate
# is a separate, smaller job — see render-decoupling.md.
BANNED=(wgpu bevy_render bevy_pbr bevy_core_pipeline egui winit)

fail=0

# PRECONDITION: the package must exist.
#
# `cargo tree -p <unknown>` exits 0 and silently IGNORES the filter, walking
# the whole workspace graph instead — which of course contains wgpu, via the
# GUI binary. So a renamed or typo'd package leaves every assertion below
# measuring the wrong graph. Today that direction happens to be loud (the
# workspace links wgpu, so it reports FAIL), but the failure is cosmetic
# luck: the same silent-filter behaviour reads as a clean PASS for any
# banned crate the workspace does not happen to contain. `cargo pkgid` is
# the invocation that actually reports an unknown package as an error.
if ! cargo pkgid -p "$PKG" >/dev/null 2>&1; then
    echo "  ERROR package '$PKG' not found in this workspace."
    echo "        The gate cannot run. If the headless server was renamed,"
    echo "        update PKG here — do not delete the check."
    exit 1
fi

echo "── render-decoupling: $PKG must link no GPU stack ──────────"
for crate in "${BANNED[@]}"; do
    out="$(cargo tree -p "$PKG" -i "$crate" -e normal 2>&1 || true)"
    # TWO distinct shapes of absence, both genuine:
    #
    #   "nothing to print"          — the crate is in the lockfile (some other
    #                                 workspace member pulls it) but nothing
    #                                 reaches it from $PKG. e.g. wgpu, which
    #                                 the GUI binary legitimately links.
    #   "did not match any packages" — the crate is not in the dependency
    #                                 graph at all. The strongest result;
    #                                 egui reports this today.
    #
    # Treating the second as an error would fail the gate at its cleanest
    # possible state. It is safe to accept here ONLY because the `cargo
    # pkgid` precondition above already proved $PKG resolves — so this string
    # can no longer be the symptom of a typo'd package under test.
    if echo "$out" | grep -qi "nothing to print\|did not match any packages"; then
        printf '  ok    %-20s absent\n' "$crate"
    elif echo "$out" | grep -qi "^error"; then
        # cargo itself failed — a bad manifest, an unresolvable lockfile, a
        # typo'd package name. Distinguished from "LINKED" deliberately: both
        # abort, but reporting a broken toolchain as an architecture
        # regression sends the reader to the wrong document entirely.
        printf '  ERROR %-20s cargo tree failed (not an architecture verdict)\n' "$crate"
        echo "$out" | head -5 | sed 's/^/        /'
        fail=1
    else
        printf '  FAIL  %-20s LINKED into the headless binary\n' "$crate"
        echo "$out" | head -20 | sed 's/^/        /'
        fail=1
    fi
done

# The rule that keeps it that way: bevy_pbr is confined to the binding layer.
#
# Two sanctioned enablers besides it, both non-negotiable and both documented:
#   * `luncosim`  — always a windowed GUI app, so the full render + windowing
#                   stack is unconditional there by design.
#   * `lunco-usd` — enables it under [dev-dependencies] ONLY, so that
#                   `cargo check -p lunco-usd --all-targets` is honest; the
#                   library itself is render-free. Without that line the
#                   examples only built under `--workspace`, where feature
#                   unification silently borrowed bevy_pbr from another crate.
#
# Detection is textual on purpose: it fires the moment a NEW crate adds the
# feature, which is a reviewable event, rather than waiting for the graph to
# already be poisoned.
echo
echo "── bevy_pbr feature is confined to the binding layer ───────"
ALLOWED_RE='lunco-render-bevy|luncosim|lunco-usd'
offenders="$(grep -l '^bevy = .*"bevy_pbr"' crates/*/Cargo.toml 2>/dev/null \
    | grep -Ev "crates/($ALLOWED_RE)/Cargo.toml" || true)"
if [ -n "$offenders" ]; then
    echo "  FAIL  a crate outside the binding layer enables bevy_pbr:"
    echo "$offenders" | sed 's/^/        /'
    echo "        Domain crates state appearance INTENT (lunco-render);"
    echo "        lunco-render-bevy is the only crate that binds a material."
    fail=1
else
    echo "  ok    no unsanctioned bevy_pbr enabler"
fi

echo
if [ "$fail" -ne 0 ]; then
    echo "── render-decoupling gate FAILED ──"
    echo "See docs/architecture/render-decoupling.md."
    exit 1
fi
echo "── render-decoupling gate passed ──"
