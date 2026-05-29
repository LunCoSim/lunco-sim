#!/bin/bash
# ============================================================================
# LunCoSim — clippy lint gate
# ============================================================================
# Runs `cargo clippy --workspace` so the `disallowed_methods` ban list in
# clippy.toml is actually enforced across EVERY member crate — not just the
# few a developer happens to `cargo clippy -p <crate>` by hand.
#
# Why this script exists (the enforcement gap it closes):
#   The bans live in clippy.toml at the workspace root and the severity is
#   `deny` (Cargo.toml `[workspace.lints.clippy]`). But a per-crate
#   `cargo clippy -p foo` only lints `foo`. The 2026-05-29 sandbox FPS
#   regression recurred a FOURTH time in `lunco-materials` precisely
#   because nobody ran clippy against that crate after the ban was added —
#   the `openusd::usda::TextReader::clone` deep-copy slipped straight past.
#   `--workspace` is the only invocation that lints every crate, so this is
#   the regression guard for the whole ban list (TextReader::clone,
#   std::fs, std::thread::spawn, Instant::now, GridAnchor re-parenting).
#
# Companion to scripts/check_wasm.sh (the wasm build gate). Same contract:
# a CI step you can also run locally before pushing.
#
# NOTE: this is a full workspace clippy pass and is BUILD-HEAVY. On a
# resource-constrained machine prefer running it once before a push rather
# than per-commit; it is deliberately NOT wired as a git hook.
#
# Usage:
#   scripts/check_lints.sh           # lint whole workspace, deny on warnings
#   scripts/check_lints.sh -p foo    # extra args pass through to cargo clippy
#
# Exit codes:
#   0  — clean (no disallowed methods, no denied lints)
#   non-zero — a banned method or denied lint was hit; output streamed
# ============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

# `-j 2` keeps peak memory/link pressure down — this machine OOMs the
# linker on wider parallelism (see memory: cargo resource use). Override
# with CARGO_BUILD_JOBS in the environment if you have the headroom.
JOBS="${CARGO_BUILD_JOBS:-2}"

echo "── cargo clippy --workspace (jobs=$JOBS) ───────────────────"
# --all-targets so tests/examples/benches are linted too; the ban list
# allows `tests/` and build scripts via local #[allow], so this is safe.
# Trailing `-D warnings` makes the `deny` severity bite even if a crate
# forgot `[lints] workspace = true`.
cargo clippy --workspace --all-targets -j "$JOBS" "$@" -- -D warnings

echo
echo "── clippy lint gate passed ──"
