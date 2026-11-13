---
name: packaged-builds
description: Build, inspect, install, or acceptance-test a packaged LunCoSim desktop release, AppImage, macOS package, Windows MSI, or Velopack update. Use when source builds are not sufficient or an installed GitHub release must be verified.
---

# Packaged desktop builds and updates

Read the packaged-build rules in [`AGENTS.md`](../../AGENTS.md), the native
builder [`scripts/build_native.sh`](../../scripts/build_native.sh), and the
application guide [`docs/apps/luncosim/README.md`](../../docs/apps/luncosim/README.md).

## Build and inspect

Before compiling LunCoSim, install the
[source-build dependencies](../../docs/apps/README.md#build-from-source),
including LLVM/libclang for native FMI bindings. These tools are needed on the
build machine, not on machines that only run the packaged application.

Use the repository's native build script and the requested runtime target. A
packaged acceptance run must use exactly one dated installer from the official
GitHub releases: Windows `LunCoSim-Windows-x86_64.msi`, macOS Apple
Silicon or Intel `.pkg`, or Linux `LunCoSim-Linux-x86_64.AppImage`. Do not use
source archives, Actions artifacts, raw archives, or a debug binary as a
packaged-release claim.

Windows MSI packaging requires an ordered three-part `--msi-version` (or
`LUNCO_MSI_VERSION`) in addition to the app's SemVer `--version`. The manual
Dev Builds workflow maps versions such as `0.6.0-dev.1` to MSI version `1.6.1`
and stable `0.6.0` to `1.6.999`. `scripts/msi_version.py` allocates the next
`dev.N`, rejects non-increasing versions within the Dev SemVer train, and keeps
the MSI version ahead of all published Windows packages.
The first `0.6.0-dev.1` release sorts below existing `0.6.0-nightly.*` builds
in SemVer, so users must install its MSI once before automatic Dev updates.

The workflow submits one ZIP containing the Windows executable and MSI to
SignPath. Its artifact configuration signs the executable, the executable
embedded in the MSI, and the MSI. It then repacks the Velopack update from the
signed executable so the full update package contains the signed binary.
`signing_mode=test` uses a test signing policy and produces Actions artifacts
only; that certificate must be trusted on the isolated test machine.
`signing_mode=off` produces unsigned sample artifacts for initial configuration.
Only `signing_mode=release` can publish the human installers and updater feed.
Production release signing runs from `main` and waits up to two hours for the
SignPath approval before downloading and verifying the signed files.

Configure these repository settings before dispatching a signed build:

- Secret: `SIGNPATH_API_TOKEN`
- Variables: `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`,
  `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`, `SIGNPATH_TEST_POLICY_SLUG`, and
  `SIGNPATH_RELEASE_POLICY_SLUG`
- Existing updater publication secret: `LUNCOSIM_UPDATES_TOKEN`

Windows packaging uses the LunCoSim Velopack fork at the revision pinned in
`.github/workflows/dev.yml`. Build its native Windows helpers in release
mode, place them in the fork's `target/debug` helper lookup directory, and
build the fork's `Velopack.Vpk` project for `net8.0` in Debug mode. Put its
`build/Debug/net8.0` directory on `PATH` before invoking the native builder.
The workflow contains the exact commands. Linux and macOS use the pinned
published Velopack CLI.

The Windows packaging script verifies that both the application executable and
the Velopack launcher contain the generated ICO frames. A missing or incorrect
resource fails the build before publication. With `LUNCOSIM_BIN` set to the
production Windows executable, run `python scripts/api/test_windows_icon.py
--port <free-port>` to verify the running window's icon and owned API shutdown.

On Linux, launching the AppImage registers a per-user application-menu entry
and icon under `$XDG_DATA_HOME` (normally `~/.local/share`) so Wayland can match
the running window to its icon. The entry points to the same writable AppImage
and is updated if it moves. An unrelated `luncosim.desktop` is never replaced.
To remove this integration after deleting the AppImage, remove
`applications/luncosim.desktop` and
`icons/hicolor/256x256/apps/io.github.luncosim.AppImage.png` under that data
directory.

Verify the artifact itself, its desktop/icon contract, launch path, scene load,
API readiness, and one relevant user workflow. Linux AppImages remain writable
and must be relaunched from that same path for updater testing. The updater
feed is machine-only; it is not the human installer source.

Windows desktop startup must not create a console by default. Check
**Settings → Debug → Show console on next start** across restarts and the
one-launch `--console` override. The preference remains enabled until switched
off. With `LUNCOSIM_BIN` set to the production Windows executable, run
`python scripts/api/test_windows_console.py --port <free-port>` to verify
native console attachment, persistence, and redirected CLI output. The script
uses isolated settings and controls only its own sessions. Also inspect the
installed shortcut launch and the Settings menu in the packaged application;
a source-build gate alone does not establish installer acceptance.

Keep the installed app separate from the user asset cache. Report `NotInstalled`
for source builds and ordinary archives when testing update behavior. Do not
lower rendering quality or disable shadows to make package acceptance pass.
