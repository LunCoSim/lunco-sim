#!/usr/bin/env bash
# Shared cache-directory resolution for repository build and integration scripts.
# Keep this in lockstep with lunco_assets::cache_dir(): explicit override first,
# then the user's shared `.lunco/cache` directory. Twin-local `.cache`
# directories are never implicit sources for build and integration scripts.

resolve_cache_dir() {
    if [[ -n "${LUNCOSIM_CACHE:-}" ]]; then
        printf '%s\n' "$LUNCOSIM_CACHE"
        return
    fi

    printf '%s\n' "${HOME:?}/.lunco/cache"
}
