#!/usr/bin/env bash
#
# Run the smallest useful Rust test command for one package.
#
# Cargo auto-discovers each `tests/*.rs` file as its own integration target.
# This wrapper keeps that fast edit-loop behavior while removing the need to
# remember the target name. It also uses sccache when it is installed.
#
#   ./scripts/run_rust_tests.sh -p lunco-modelica --module rumoca_api_coverage
#   ./scripts/run_rust_tests.sh -p lunco-usd --filter integration_asset_loading::test_sandbox_scene_composes
#   ./scripts/run_rust_tests.sh -p lunco-scripting --check --module rhai_test_harness
#   ./scripts/run_rust_tests.sh -p lunco-modelica --lib --filter runtime_telemetry::tests

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PACKAGE=""
TARGET=""
MODULE=""
FILTER=""
CHECK=0
NO_RUN=0
LIST=0
RELEASE=0
LIB=0
JOBS=4
FEATURES=""
HARNESS_ARGS=()

usage() {
    sed -n '3,12p' "${BASH_SOURCE[0]}" | sed '/^#$/d; s/^# //'
    cat <<'EOF'

Options:
  -p, --package NAME       Cargo package name (required)
      --module NAME        Select tests/NAME.rs
      --file PATH           Select an integration test by its file basename
      --target NAME         Select an explicit Cargo test target
      --filter TEXT          Filter a test; module::test selects that module file
      --lib                  Select the package library's unit-test target
      --check                Compile the selected target with cargo check
      --no-run               Build the selected test target, but do not execute
      --list                 List tests instead of running them
      --release              Use Cargo's release test profile
  -j, --jobs N              Cargo parallelism (default: 4)
      --features FEATURES   Cargo features to enable
      -- [ARGS...]           Arguments passed to the Rust test harness
  -h, --help                Show this help
EOF
}

die() {
    echo "$*" >&2
    exit 2
}

while (($# > 0)); do
    case "$1" in
        -p|--package)
            (($# >= 2)) || die "$1 needs a package name"
            PACKAGE="$2"
            shift 2
            ;;
        --module)
            (($# >= 2)) || die "--module needs a module name"
            [[ -z "$MODULE" && -z "$TARGET" ]] || die "only one module/file/target selector is supported"
            MODULE="${2%.rs}"
            shift 2
            ;;
        --file)
            (($# >= 2)) || die "--file needs a path"
            [[ -z "$MODULE" && -z "$TARGET" ]] || die "only one module/file/target selector is supported"
            MODULE="$(basename "${2%.rs}")"
            shift 2
            ;;
        --target)
            (($# >= 2)) || die "--target needs a Cargo test target name"
            [[ -z "$MODULE" && -z "$TARGET" ]] || die "only one module/file/target selector is supported"
            TARGET="$2"
            shift 2
            ;;
        --filter)
            (($# >= 2)) || die "--filter needs text"
            [[ -z "$FILTER" ]] || die "only one filter is supported"
            FILTER="$2"
            shift 2
            ;;
        --lib)
            LIB=1
            shift
            ;;
        --check)
            CHECK=1
            shift
            ;;
        --no-run)
            NO_RUN=1
            shift
            ;;
        --list)
            LIST=1
            shift
            ;;
        --release)
            RELEASE=1
            shift
            ;;
        -j|--jobs)
            (($# >= 2)) || die "$1 needs a number"
            JOBS="$2"
            shift 2
            ;;
        --features)
            (($# >= 2)) || die "--features needs a value"
            FEATURES="$2"
            shift 2
            ;;
        --)
            shift
            HARNESS_ARGS=("$@")
            break
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --*)
            die "unknown option: $1"
            ;;
        *)
            [[ -z "$FILTER" ]] || die "only one filter is supported"
            FILTER="$1"
            shift
            ;;
    esac
done

[[ -n "$PACKAGE" ]] || die "--package is required"
[[ "$CHECK" == 0 || "$NO_RUN" == 0 ]] || die "--check and --no-run are mutually exclusive"
[[ "$CHECK" == 0 || "$LIST" == 0 ]] || die "--check and --list are mutually exclusive"
[[ "$LIST" == 0 || "$NO_RUN" == 0 ]] || die "--list and --no-run are mutually exclusive"
[[ ${#HARNESS_ARGS[@]} == 0 || "$CHECK" == 0 ]] || die "harness arguments cannot be used with --check"
[[ "$LIB" == 0 || -z "$MODULE" && -z "$TARGET" ]] || die "--lib cannot be combined with an integration target selector"

PACKAGE_DIR="crates/$PACKAGE"
[[ -f "$PACKAGE_DIR/Cargo.toml" ]] || die "unknown package directory: $PACKAGE_DIR"

RUN_FILTER="$FILTER"
if [[ -n "$MODULE" ]]; then
    TARGET="$MODULE"
    [[ -f "$PACKAGE_DIR/tests/$TARGET.rs" ]] || die "integration test source not found: $PACKAGE_DIR/tests/$TARGET.rs"
    if [[ -n "$FILTER" && "$FILTER" == "$TARGET"::* ]]; then
        RUN_FILTER="${FILTER#*::}"
    fi
elif [[ -n "$FILTER" && "$LIB" == 0 && -z "$TARGET" ]]; then
    if [[ "$FILTER" == *::* ]]; then
        candidate="${FILTER%%::*}"
        if [[ -f "$PACKAGE_DIR/tests/$candidate.rs" ]]; then
            TARGET="$candidate"
            RUN_FILTER="${FILTER#*::}"
        else
            die "cannot map filter '$FILTER' to tests/$candidate.rs; use --target"
        fi
    else
        die "--filter needs module::test so the runner can select one target; use --module for a whole file"
    fi
elif [[ -n "$FILTER" && -n "$TARGET" ]]; then
    candidate="${FILTER%%::*}"
    if [[ "$candidate" == "$TARGET" ]]; then
        RUN_FILTER="${FILTER#*::}"
    fi
fi

[[ "$CHECK" == 0 || -z "$RUN_FILTER" ]] || die "--check compiles the whole selected target; use --no-run for a filtered test build"

CARGO_ARGS=()
if ((CHECK)); then
    CARGO_ARGS=(check -p "$PACKAGE")
else
    CARGO_ARGS=(test -p "$PACKAGE")
fi
if ((LIB)); then
    CARGO_ARGS+=(--lib)
elif [[ -n "$TARGET" ]]; then
    CARGO_ARGS+=(--test "$TARGET")
else
    # No selector is deliberately the package-level integration gate. The
    # development path should always pass --module, --file, or --filter.
    CARGO_ARGS+=(--tests)
fi
if [[ -n "$FEATURES" ]]; then
    CARGO_ARGS+=(--features "$FEATURES")
fi
if ((RELEASE)); then
    CARGO_ARGS+=(--release)
fi
CARGO_ARGS+=(-j "$JOBS")
if ((NO_RUN)); then
    CARGO_ARGS+=(--no-run)
fi
if [[ -n "$RUN_FILTER" && "$CHECK" == 0 ]]; then
    CARGO_ARGS+=("$RUN_FILTER")
fi
if ((LIST)); then
    CARGO_ARGS+=(-- --list)
elif ((${#HARNESS_ARGS[@]} > 0)); then
    CARGO_ARGS+=(-- "${HARNESS_ARGS[@]}")
fi

if [[ -z "${RUSTC_WRAPPER+x}" ]] && command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER=sccache
fi

exec cargo "${CARGO_ARGS[@]}"
