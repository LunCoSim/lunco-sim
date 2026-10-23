#!/usr/bin/env bash
# Link the read-only sample exporter against the adjacent, already-built Tracy server.
set -euo pipefail
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ $# != 1 ]]; then
    printf 'Usage: bash scripts/perf/build_tracy_samples.sh MATCHING_TRACY_SOURCE\n' >&2
    exit 1
fi
tracy_root="$(cd -- "$1" && pwd)"
tracy_build="$tracy_root/csvexport/build"
sort_source="$(sed -n 's/^PPQSort_SOURCE_DIR:STATIC=//p' "$tracy_build/CMakeCache.txt")"
if [[ ! -f "$sort_source/include/ppqsort.h" ]]; then
    printf 'The adjacent Tracy build has no configured PPQSort headers.\n' >&2
    exit 1
fi
for library in "$tracy_build/libTracyServer.a" \
    "$tracy_build/_deps/capstone-build/libcapstone.a" \
    "$tracy_build/_deps/zstd-build/lib/libzstd.a"; do
    if [[ ! -f "$library" ]]; then
        printf 'Missing %s; build the matching adjacent Tracy CSV exporter first.\n' "$library" >&2
        exit 1
    fi
done
mkdir -p "$task_root/target/perf"
sccache "${CXX:-c++}" -std=c++20 -O2 -DNDEBUG -Wall -Wextra -flto=4 \
    -isystem "$tracy_root/server" -isystem "$tracy_root/public/common" -isystem "$sort_source/include" \
    "$task_root/scripts/perf/tracy_samples.cpp" \
    "$tracy_build/libTracyServer.a" \
    "$tracy_build/_deps/capstone-build/libcapstone.a" \
    "$tracy_build/_deps/zstd-build/lib/libzstd.a" \
    -pthread -ldl -o "$task_root/target/perf/tracy-samples"
printf '%s\n' "$task_root/target/perf/tracy-samples"
