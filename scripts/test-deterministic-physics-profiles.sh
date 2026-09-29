#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${LUNCOSIM_BIN:-$root/target/debug/luncosim}"
reference="${LUNCOSIM_DETERMINISM_REFERENCE:-$root/scripts/tests/fixtures/deterministic-physics-reference.json}"

if [[ "$binary" == */* ]]; then
    [[ -x "$binary" ]] || { printf 'luncosim binary is not executable: %s\n' "$binary" >&2; exit 2; }
else
    command -v "$binary" >/dev/null || { printf 'luncosim binary was not found: %s\n' "$binary" >&2; exit 2; }
fi
[[ -f "$reference" ]] || { printf 'determinism reference was not found: %s\n' "$reference" >&2; exit 2; }

cd "$root"

run_profile() {
    local name="$1" scene="$2" threads="$3" jitter="$4" seed="$5"
    printf '\nDeterminism profile: %s\n' "$name"
    LUNCO_ASSET_ROOT="$root/assets" "$binary" test \
        --scene "$scene" \
        --threads "$threads" \
        --jitter "$jitter" \
        --seed "$seed" \
        --determinism-reference "$reference"
}

seed=6840157149251759617
run_profile scene-4-serial assets/scenes/tests/multi_rover_stress_4.usda 1 0.0 "$seed"
run_profile scene-4-default assets/scenes/tests/multi_rover_stress_4.usda 0 0.0 "$seed"
run_profile scene-8-serial assets/scenes/tests/multi_rover_stress_8.usda 1 0.0 "$seed"
run_profile scene-8-default assets/scenes/tests/multi_rover_stress_8.usda 0 0.0 "$seed"
run_profile scene-20-serial assets/scenes/tests/multi_rover_stress_20.usda 1 0.0 "$seed"
run_profile scene-20-default assets/scenes/tests/multi_rover_stress_20.usda 0 0.0 "$seed"
run_profile jitter-0.25-seed-6840157149251759617 assets/scenes/tests/multi_rover_stress_4.usda 1 0.25 6840157149251759617
run_profile jitter-0.25-seed-1234567890123456789 assets/scenes/tests/multi_rover_stress_4.usda 1 0.25 1234567890123456789
run_profile jitter-0.5-seed-6840157149251759617 assets/scenes/tests/multi_rover_stress_4.usda 1 0.5 6840157149251759617
run_profile jitter-0.5-seed-1234567890123456789 assets/scenes/tests/multi_rover_stress_4.usda 1 0.5 1234567890123456789

printf '\nDETERMINISTIC_PHYSICS_PROFILES_OK\n'
