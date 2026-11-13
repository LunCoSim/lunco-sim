from pathlib import Path
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).parents[2]


def read_nightly_workflow() -> str:
    workflow_path = Path(
        os.environ.get(
            "LUNCO_NIGHTLY_WORKFLOW_PATH",
            str(ROOT / ".github/workflows/nightly.yml"),
        )
    )
    if not workflow_path.is_file():
        raise unittest.SkipTest("nightly workflow is maintained in its own repository")
    return workflow_path.read_text(encoding="utf-8")


class NightlyReleaseContractTests(unittest.TestCase):
    def test_velopack_arguments_support_empty_options_under_nounset(self) -> None:
        builder = (ROOT / "scripts/build_native.sh").read_text(encoding="utf-8")
        script = builder.split("    VPK_ARGS=(pack \\\n", 1)[1]
        script = "VPK_ARGS=(pack \\\n" + script.split('    vpk "${VPK_ARGS[@]}"', 1)[0]
        script += '    vpk "${VPK_ARGS[@]}"\n'
        harness = 'set -eu\ninfo() { :; }\nvpk() { printf "%s\\n" "$@"; }\n' + script
        for platform, icon in (
            ("macos", ""),
            ("macos", "icon with spaces.icns"),
            ("linux", "icon.png"),
            ("windows", "icon.ico"),
        ):
            with self.subTest(platform=platform, icon=icon):
                env = dict(
                    os.environ, PLATFORM=platform, PACKAGE_ICON=icon,
                    MSI_VERSION="1.0.15101", VPK_PACK_ID="luncosim",
                    RELEASE_VERSION="0.6.0-nightly.151.1", VPK_CHANNEL="test-channel",
                    VPK_RUNTIME="test-runtime", OUT_DIR="folder with spaces",
                    BIN_NAME="luncosim", VELOPACK_OUT="output folder",
                )
                bash = "/bin/bash" if sys.platform == "darwin" else "bash"
                result = subprocess.run([bash, "-c", harness], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                args = result.stdout.splitlines()
                self.assertEqual(args[args.index("--packDir") + 1], "folder with spaces")
                self.assertEqual(args[args.index("--outputDir") + 1], "output folder")
                self.assertEqual("--msi" in args, platform == "windows")
                if platform == "windows":
                    self.assertEqual(args[args.index("--instLocation") + 1], "Either")
                    self.assertEqual(args[args.index("--msiVersion") + 1], "1.0.15101")
                self.assertEqual("--icon" in args, bool(icon))
                if icon:
                    self.assertEqual(args[args.index("--icon") + 1], icon)

    def test_version_generation_rejects_failed_or_missing_metadata(self) -> None:
        workflow = read_nightly_workflow()
        step = workflow.split("      - id: version\n", 1)[1]
        script = textwrap.dedent(
            step.split("        run: |\n", 1)[1].split("\n  #", 1)[0]
        )
        cases = [
            (0, [{"name": "lunco-luncosim", "version": "0.1.0-dev"}], True),
            (101, [], False),
            (0, [], False),
        ]
        for cargo_status, packages, succeeds in cases:
            with self.subTest(cargo_status=cargo_status, packages=packages):
                with tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    cargo = root / "cargo"
                    cargo.write_text(
                        '#!/bin/sh\nprintf "%s\\n" "$TEST_METADATA"\n'
                        'exit "$TEST_CARGO_STATUS"\n'
                    )
                    cargo.chmod(0o755)
                    output = root / "output"
                    env = dict(
                        os.environ,
                        PATH=f"{root}:{os.environ['PATH']}",
                        TEST_METADATA=json.dumps({"packages": packages}),
                        TEST_CARGO_STATUS=str(cargo_status),
                        GITHUB_OUTPUT=str(output),
                        RUN_NUMBER="139",
                        RUN_ATTEMPT="1",
                    )
                    result = subprocess.run(
                        ["bash", "-e", "-c", script], env=env,
                        capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode == 0, succeeds, result.stderr)
                    if succeeds:
                        self.assertEqual(
                            output.read_text(),
                            "version=0.1.0-nightly.139.1\nmsi_version=1.0.13901\nrun_number=139\n",
                        )
                    else:
                        self.assertFalse(output.exists())

    def test_nightly_workflow_publishes_only_in_updates_repo_from_pinned_source(self) -> None:
        workflow = read_nightly_workflow()

        self.assertIn("repository: LunCoSim/lunco-sim", workflow)
        self.assertIn("ref: main", workflow)
        self.assertIn("source_sha: ${{ steps.source.outputs.sha }}", workflow)
        self.assertGreaterEqual(
            workflow.count("ref: ${{ needs.meta.outputs.source_sha }}"), 2
        )
        self.assertIn(
            "LUNCO_REPOSITORY_URL: https://github.com/LunCoSim/lunco-sim", workflow
        )
        self.assertIn('UPDATES_REPOSITORY: LunCoSim/lunco-sim-updates', workflow)
        self.assertIn(
            'if [[ "$GITHUB_REPOSITORY" != "$UPDATES_REPOSITORY" ]]; then', workflow
        )
        self.assertEqual(workflow.count('gh release create "$RELEASE_TAG"'), 1)
        self.assertIn('--repo "$UPDATES_REPOSITORY"', workflow)
        self.assertIn('--prerelease', workflow)
        self.assertIn('--latest=false', workflow)
        self.assertNotIn("MAIN_GH_TOKEN", workflow)
        self.assertIn("--format release-notes", workflow)
        self.assertIn("Source commit:", workflow)

    def test_release_identity_is_explicit_and_fail_closed(self) -> None:
        workflow = read_nightly_workflow()

        self.assertIn("release_tag: ${{ steps.metadata.outputs.release_tag }}", workflow)
        self.assertIn("RELEASE_TAG: ${{ needs.meta.outputs.release_tag }}", workflow)
        self.assertIn("Invalid nightly release tag", workflow)
        self.assertIn('if [ -z "$RELEASE_TAG" ]; then', workflow)
        self.assertIn("refusing to replace it", workflow)
        self.assertIn("--target main", workflow)

        guard = workflow.index('if [ -z "$RELEASE_TAG" ]; then')
        first_release_create = workflow.index('gh release create "$RELEASE_TAG"')
        self.assertLess(guard, first_release_create)

    def test_application_reads_the_machine_only_repository(self) -> None:
        updater = (
            ROOT / "crates/lunco-updater/src/lib.rs"
        ).read_text(encoding="utf-8")

        self.assertIn(
            '"https://github.com/LunCoSim/lunco-sim-updates"', updater
        )
        self.assertIn("TimeoutGithubSource::new(UPDATE_REPOSITORY, true,", updater)
        self.assertIn("UPDATE_HTTP_TIMEOUT", updater)
        self.assertIn('.header("Range", &range)', updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "linux-x64";', updater)
        self.assertIn("Download update", updater)
        self.assertIn("Install and restart", updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "win-x64";', updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "osx-x64";', updater)
        self.assertIn('const UPDATE_CHANNEL: &str = "osx-arm64";', updater)
        self.assertNotIn("let source = GithubSource::new", updater)
        self.assertNotIn("HttpSource", updater)

    def test_linux_download_explains_the_update_managed_appimage_path(self) -> None:
        app_guide = (ROOT / "docs/apps/luncosim/README.md").read_text(encoding="utf-8")
        package_builder = (ROOT / "scripts/build_native.sh").read_text(encoding="utf-8")

        for text in (app_guide, package_builder):
            self.assertIn(".AppImage", text)
            self.assertIn("writable", text)
            self.assertIn("same AppImage", text)
            self.assertIn("Download update", text)
            self.assertIn("Install and restart", text)

        self.assertIn("LunCoSim/lunco-sim-updates", app_guide)
        self.assertIn("LunCoSim-Windows-x86_64.msi", app_guide)
        self.assertIn("LunCoSim-macOS-Apple-Silicon.pkg", app_guide)
        self.assertIn("LunCoSim-macOS-Intel.pkg", app_guide)

    def test_native_package_passes_the_platform_icon_to_velopack(self) -> None:
        package_builder = (ROOT / "scripts/build_native.sh").read_text(encoding="utf-8")
        icon_builder = (ROOT / "crates/lunco-luncosim-ui/build.rs").read_text(encoding="utf-8")

        self.assertIn('LUNCOSIM_ICON_OUTPUT_DIR="$ICON_OUTPUT_DIR"', package_builder)
        self.assertIn("cargo build", package_builder)
        self.assertIn('PACKAGE_FEATURE_ARGS+=(--features package-icons,updates)', package_builder)
        self.assertIn('LUNCOSIM_ICON_OUTPUT_STAMP="$ICON_OUTPUT_STAMP"', package_builder)
        self.assertIn("prepare_package_icon", package_builder)
        self.assertIn('VPK_ARGS+=(--icon "$PACKAGE_ICON")', package_builder)
        self.assertIn('iconutil -c icns', package_builder)
        self.assertIn("write_windows_ico", icon_builder)
        self.assertIn("cargo::metadata=icon_path=", icon_builder)
        self.assertIn("verify_windows_icons.py", package_builder)
        self.assertIn("write_macos_iconset", icon_builder)
        self.assertIn("write_linux_icons", icon_builder)
        self.assertIn("LUNCOSIM_ICON_OUTPUT_DIR", icon_builder)
        self.assertIn("LUNCOSIM_ICON_OUTPUT_STAMP", icon_builder)

    def test_linux_package_identity_reaches_the_compiled_window_and_final_appimage(self) -> None:
        package_builder = (ROOT / "scripts/build_native.sh").read_text(encoding="utf-8")
        window_source = (ROOT / "crates/lunco-luncosim-ui/src/application.rs").read_text(encoding="utf-8")
        verifier = (ROOT / "scripts/verify_linux_appimage.sh").read_text(encoding="utf-8")

        self.assertIn('VPK_PACK_ID="luncosim"', package_builder)
        self.assertIn('window.name = Some("luncosim".to_string());', window_source)
        self.assertIn('verify_linux_appimage "$VELOPACK_OUT"', package_builder)
        self.assertNotIn('cat > "$dir/LunCoSim.desktop"', package_builder)
        self.assertIn('"$appimage" --appimage-extract', verifier)
        self.assertIn('find "$appdir" -type f -name \'*.desktop\'', verifier)
        self.assertIn('desktop filename does not match its Icon', verifier)
        self.assertIn('StartupWMClass', verifier)
        self.assertIn('.DirIcon', verifier)


if __name__ == "__main__":
    unittest.main()
