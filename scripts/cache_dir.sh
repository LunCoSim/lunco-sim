#!/usr/bin/env bash
# Shared cache-directory resolution for repository build and integration scripts.
# Keep this in lockstep with lunco_assets::cache_dir(): explicit override first,
# then the conventional machine-global cache. A worktree-local cache is never
# an implicit source.

resolve_cache_dir() {
    if [[ -n "${LUNCOSIM_CACHE:-}" ]]; then
        printf '%s\n' "$LUNCOSIM_CACHE"
        return
    fi

    case "$(uname -s)" in
        Darwin*) printf '%s\n' "${HOME:?}/Library/Caches/lunco" ;;
        MINGW*|MSYS*|CYGWIN*)
            printf '%s\n' "${LOCALAPPDATA:-${HOME:?}/AppData/Local}/lunco"
            ;;
        *) printf '%s\n' "${XDG_CACHE_HOME:-${HOME:?}/.cache}/lunco" ;;
    esac
}
