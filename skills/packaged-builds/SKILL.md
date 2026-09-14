---
name: packaged-builds
description: Build, inspect, install, or acceptance-test a packaged LunCoSim desktop release, AppImage, macOS package, Windows Setup.exe, or Velopack update. Use when source builds are not sufficient or an installed GitHub release must be verified.
---

# Packaged desktop builds and updates

Read the packaged-build rules in [`AGENTS.md`](../../AGENTS.md), the native
builder [`scripts/build_native.sh`](../../scripts/build_native.sh), and the
application guide [`docs/apps/luncosim/README.md`](../../docs/apps/luncosim/README.md).

## Build and inspect

Use the repository's native build script and the requested runtime target. A
packaged acceptance run must use exactly one dated installer from the official
GitHub releases: Windows `LunCoSim-Windows-x86_64-Setup.exe`, macOS Apple
Silicon or Intel `.pkg`, or Linux `LunCoSim-Linux-x86_64.AppImage`. Do not use
source archives, Actions artifacts, raw archives, or a debug binary as a
packaged-release claim.

Verify the artifact itself, its desktop/icon contract, launch path, scene load,
API readiness, and one relevant user workflow. Linux AppImages remain writable
and must be relaunched from that same path for updater testing. The updater
feed is machine-only; it is not the human installer source.

Keep the installed app separate from the user asset cache. Report `NotInstalled`
for source builds and ordinary archives when testing update behavior. Do not
lower rendering quality or disable shadows to make package acceptance pass.
