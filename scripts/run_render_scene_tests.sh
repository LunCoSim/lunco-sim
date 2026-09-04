#!/usr/bin/env bash
#
# Run the GPU-backed scene tests whose Rhai observer declares `TEST_KIND =
# "graphics"`.
# The production luncosim binary is still the test subject: this script only
# supplies the offscreen capture contract and checks the authored render
# expectations against the frames and diagnostics it produced.
#
# The headless gate calls this script after building target/debug/luncosim. It
# can also be run directly after that binary has been built:
#
#   ./scripts/run_render_scene_tests.sh
#   ./scripts/run_render_scene_tests.sh hdri
#   ./scripts/run_render_scene_tests.sh --exact hdri
#   RENDER_QUALITY=balanced ./scripts/run_render_scene_tests.sh hdri
#
# RENDER_FRAMES is intentionally small. The render assertions are about settled
# scene state, not an animation sequence; the recorder's readiness gate waits
# for asynchronous visual assets before these frames begin.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT" || exit 1

BIN="${LUNCOSIM_BIN:-target/debug/luncosim}"
RENDER_TIMEOUT="${RENDER_TIMEOUT:-120}"
RENDER_FRAMES="${RENDER_FRAMES:-3}"
RENDER_SIZE="${RENDER_SIZE:-320x180}"
RENDER_QUALITY="${RENDER_QUALITY:-high}"
RENDER_API_PORT="${RENDER_API_PORT:-4103}"
RUN_ID="$(date +%Y%m%dT%H%M%S)-$$"
LOG_DIR="target/scene-tests"
RENDER_ROOT="$LOG_DIR/render"

if [[ ! -x "$BIN" ]]; then
    echo "render scene gate: production binary is missing or not executable: $BIN" >&2
    echo "build it first with: RUSTC_WRAPPER=sccache cargo build -j 4 -p lunco-luncosim --bin luncosim" >&2
    exit 2
fi

if ! [[ "$RENDER_FRAMES" =~ ^[1-9][0-9]*$ ]]; then
    echo "render scene gate: RENDER_FRAMES must be a positive integer" >&2
    exit 2
fi
case "$RENDER_QUALITY" in
    low|balanced|high) ;;
    *)
        echo "render scene gate: RENDER_QUALITY must be low, balanced, or high" >&2
        exit 2
        ;;
esac
if ! [[ "$RENDER_API_PORT" =~ ^[1-9][0-9]*$ && "$RENDER_API_PORT" -le 65535 ]]; then
    echo "render scene gate: RENDER_API_PORT must be a valid TCP port" >&2
    exit 2
fi
if [[ "$RENDER_SIZE" != *x* ]]; then
    echo "render scene gate: RENDER_SIZE must be WxH" >&2
    exit 2
fi
RENDER_WIDTH="${RENDER_SIZE%x*}"
RENDER_HEIGHT="${RENDER_SIZE#*x}"
if ! [[ "$RENDER_WIDTH" =~ ^[1-9][0-9]*$ && "$RENDER_HEIGHT" =~ ^[1-9][0-9]*$ ]]; then
    echo "render scene gate: RENDER_SIZE must contain positive dimensions" >&2
    exit 2
fi

# Pixel assertions use ImageMagick's image decoder, not a guessed file-size
# threshold. A PNG can be large and still be a black frame; these checks inspect
# the actual rendered pixels. The render gate is intentionally explicit about
# this host prerequisite instead of silently downgrading to "file exists".
if ! command -v identify >/dev/null 2>&1 || ! command -v convert >/dev/null 2>&1; then
    echo "render scene gate: ImageMagick (identify + convert) is required for pixel assertions" >&2
    exit 2
fi

FILTER=""
EXACT=0
while (($# > 0)); do
    case "$1" in
        --exact)
            EXACT=1
            shift
            ;;
        --*)
            echo "render scene gate: unknown option: $1" >&2
            exit 2
            ;;
        *)
            if [[ -n "$FILTER" ]]; then
                echo "render scene gate: only one scene filter is supported" >&2
                exit 2
            fi
            FILTER="$1"
            shift
            ;;
    esac
done
if ((EXACT)) && [[ -z "$FILTER" ]]; then
    echo "render scene gate: --exact needs a scene name or path" >&2
    exit 2
fi
LIST_OUTPUT="$("$BIN" test --list)" || {
    echo "render scene gate: scene test discovery failed" >&2
    exit 2
}
SCENES=()
while IFS=$'\t' read -r kind scene; do
    [[ "$kind" == "graphics" && -n "${scene:-}" ]] || continue
    SCENES+=("$scene")
done <<< "$LIST_OUTPUT"
if [[ -n "$FILTER" ]]; then
    filtered=()
    for scene in "${SCENES[@]}"; do
        if ((EXACT)); then
            [[ "$(basename "$scene" .usda)" == "$FILTER" || "$scene" == "$FILTER" ]] && filtered+=("$scene")
        else
            [[ "$scene" == *"$FILTER"* ]] && filtered+=("$scene")
        fi
    done
    SCENES=("${filtered[@]}")
fi
if [[ ${#SCENES[@]} -eq 0 ]]; then
    echo "render scene gate: no graphics scene matches '${FILTER:-all}'" >&2
    exit 2
fi

overall=0
passed=0
for scene in "${SCENES[@]}"; do
    name="$(basename "$scene" .usda)"
    output="$RENDER_ROOT/${name}-${RUN_ID}"
    log="$LOG_DIR/${name}.render.log"
    mkdir -p "$output"

    echo "==> render $name"
    timeout --kill-after=10 "$RENDER_TIMEOUT" \
        "$BIN" --api "$RENDER_API_PORT" --offscreen \
        --render-quality "$RENDER_QUALITY" \
        --record-offline "$output" \
        --record-frames "$RENDER_FRAMES" \
        --record-size "$RENDER_SIZE" \
        --scene "$scene" >"$log" 2>&1
    code=$?

    expected_last="$(printf 'frame_%06d.png' $((RENDER_FRAMES - 1)))"
    frame_count="$(find "$output" -maxdepth 1 -type f -name 'frame_*.png' | wc -l)"
    first="$output/frame_000000.png"
    last="$output/$expected_last"
    reason=""

    if [[ $code -ne 0 ]]; then
        reason="production luncosim exited $code"
    elif grep -Eqi 'panicked|Encountered a panic|thread .*panicked' "$log"; then
        reason="production render reported a panic"
    elif ! grep -Fq "recording drained (${RENDER_FRAMES} frames)" "$log"; then
        reason="recording did not drain exactly ${RENDER_FRAMES} frames"
    elif [[ "$frame_count" -ne "$RENDER_FRAMES" || ! -s "$first" || ! -s "$last" ]]; then
        reason="expected ${RENDER_FRAMES} non-empty PNG frames, found ${frame_count}"
    elif ! file "$last" | grep -Fq 'PNG image data'; then
        reason="final output is not a PNG image"
    fi

    if [[ -z "$reason" && "$name" == "hdri" ]]; then
        if ! grep -Fq 'dome cubemap ready' "$log"; then
            reason="HDRI scene never completed its DomeLight cubemap projection"
        else
            # Inspect only the upper sky band. The previous half-frame mean could
            # pass on a bright ground horizon while the actual DomeLight sky was
            # still the uniform clear color. A textured equirectangular sky must
            # contribute tonal variation before the HDRI visual review is valid.
            sky_height=$((RENDER_HEIGHT / 4))
            read -r sky_mean sky_std < <(
                convert "$last" -crop "${RENDER_WIDTH}x${sky_height}+0+0" \
                    -format '%[fx:mean] %[fx:standard_deviation]' info: 2>/dev/null
            )
            if ! awk -v mean="${sky_mean:-0}" -v std="${sky_std:-0}" \
                'BEGIN { exit !(mean + 0.0 > 0.01 && std + 0.0 > 0.01) }'; then
                reason="HDRI sky band is black or uniform (mean=${sky_mean:-unavailable}, std=${sky_std:-unavailable})"
            fi
        fi
    fi

    if [[ -z "$reason" && "$name" == "shader_fallback" ]]; then
        warning_count="$(grep -Fc 'has no `@fragment` entry point' "$log")"
        if [[ "$warning_count" -ne 1 ]]; then
            reason="expected exactly one invalid-shader warning, found ${warning_count}"
        else
            read -r red green blue < <(identify -format '%[fx:mean.r] %[fx:mean.g] %[fx:mean.b]' "$last")
            if ! awk -v r="$red" -v g="$green" -v b="$blue" \
                'BEGIN { exit !(r > 0.02 && r > g * 1.15 && r > b * 1.15) }'; then
                reason="fallback cube pixels are not visibly red (rgb=${red:-?},${green:-?},${blue:-?})"
            fi
        fi
    fi

    if [[ -z "$reason" && "$name" =~ ^(six_wheel_visual|rocker_bogie_antenna_visual|parts_attached_visual)$ ]]; then
        # These fixtures are authored visual reviews, so a successful process
        # and a PNG are not enough: reject black or empty captures by checking
        # a subject-centred region for real tonal structure.
        roi_width=$((RENDER_WIDTH / 2))
        roi_height=$((RENDER_HEIGHT * 2 / 3))
        roi_x=$((RENDER_WIDTH / 4))
        roi_y=$((RENDER_HEIGHT / 6))
        read -r subject_mean subject_std < <(
            convert "$last" \
                -crop "${roi_width}x${roi_height}+${roi_x}+${roi_y}" \
                -colorspace Gray \
                -format '%[fx:mean] %[fx:standard_deviation]' info: 2>/dev/null
        )
        if ! awk -v mean="${subject_mean:-0}" -v std="${subject_std:-0}" \
            'BEGIN { exit !(mean + 0.0 > 0.08 && std + 0.0 > 0.025) }'; then
            reason="subject region is black or visually flat (mean=${subject_mean:-unavailable}, std=${subject_std:-unavailable})"
        else
            echo "    pixels — subject mean=${subject_mean} std=${subject_std}"
        fi
    fi

    if [[ -n "$reason" ]]; then
        overall=1
        echo "    FAIL — $reason"
        tail -20 "$log" | sed 's/^/    | /'
    else
        passed=$((passed + 1))
        echo "    PASS — ${frame_count} production GPU frames; log=$log; output=$output"
    fi
done

echo "render scene gate: ${passed}/${#SCENES[@]} passed"
if [[ $overall -ne 0 ]]; then
    exit 1
fi
