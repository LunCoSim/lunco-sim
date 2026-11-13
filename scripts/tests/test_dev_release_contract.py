from pathlib import Path
import os
import subprocess
import sys
import unittest
import xml.etree.ElementTree as ET


ROOT = Path(__file__).parents[2]
SIGNPATH_NS = "{http://signpath.io/artifact-configuration/v1}"


class DevReleaseContractTests(unittest.TestCase):
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
                    os.environ,
                    PLATFORM=platform,
                    PACKAGE_ICON=icon,
                    MSI_VERSION="1.6.1",
                    WITH_MSI="1",
                    VPK_PACK_ID="luncosim",
                    RELEASE_VERSION="0.6.0-dev.1",
                    VPK_CHANNEL="test-channel",
                    VPK_RUNTIME="test-runtime",
                    OUT_DIR="folder with spaces",
                    BIN_NAME="luncosim",
                    VELOPACK_OUT="output folder",
                )
                bash = "/bin/bash" if sys.platform == "darwin" else "bash"
                result = subprocess.run(
                    [bash, "-c", harness], env=env, capture_output=True, text=True
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                args = result.stdout.splitlines()
                self.assertEqual(args[args.index("--packDir") + 1], "folder with spaces")
                self.assertEqual(args[args.index("--outputDir") + 1], "output folder")
                self.assertEqual("--msi" in args, platform == "windows")
                if platform == "windows":
                    self.assertEqual(args[args.index("--instLocation") + 1], "Either")
                    self.assertEqual(args[args.index("--msiVersion") + 1], "1.6.1")
                self.assertEqual("--icon" in args, bool(icon))
                if icon:
                    self.assertEqual(args[args.index("--icon") + 1], icon)

    def test_dev_build_is_manual_and_uses_dev_semver(self) -> None:
        workflow = (ROOT / ".github/workflows/dev.yml").read_text(encoding="utf-8")

        self.assertIn("name: Dev Builds", workflow)
        self.assertIn("  workflow_dispatch:", workflow)
        self.assertNotIn("  schedule:", workflow)
        self.assertIn("0.6.0-dev.1", workflow)
        self.assertIn('base_version="${base_version%-dev}"', workflow)
        self.assertIn('scripts/msi_version.py --next-version "$base_version"', workflow)
        self.assertIn('scripts/msi_version.py --check-published "$version"', workflow)
        self.assertIn("Release versions are allocated automatically", workflow)
        self.assertIn("LunCoSim/lunco-sim-updates", workflow)
        self.assertIn("if: needs.meta.outputs.signing_mode == 'release' && inputs.make_release", workflow)
        self.assertIn("contents: read", workflow)
        self.assertIn("contents: write", workflow)
        self.assertIn("Test/off signing modes only create Actions artifacts", workflow)
        self.assertIn("Published releases must be dispatched from the main branch", workflow)

    def test_one_signpath_request_covers_exe_msi_and_velopack_payload(self) -> None:
        workflow = (ROOT / ".github/workflows/dev.yml").read_text(encoding="utf-8")
        configuration = ET.parse(ROOT / "signing/signpath-windows.xml").getroot()

        self.assertEqual(
            workflow.count("uses: signpath/github-action-submit-signing-request@v3"), 1
        )
        self.assertIn("uses: actions/upload-artifact@v4", workflow)
        self.assertIn("steps.upload-signpath-input.outputs.artifact-id", workflow)
        self.assertIn("wait-for-completion-timeout-in-seconds: 7200", workflow)
        self.assertEqual(configuration.tag, SIGNPATH_NS + "artifact-configuration")
        bundle = configuration.find(SIGNPATH_NS + "zip-file")
        self.assertIsNotNone(bundle)
        assert bundle is not None
        standalone_exe = bundle.find(SIGNPATH_NS + "pe-file")
        msi = bundle.find(SIGNPATH_NS + "msi-file")
        self.assertEqual(standalone_exe.attrib["path"], "luncosim.exe")
        self.assertEqual(msi.attrib["path"], "LunCoSim-Windows-x86_64.msi")
        self.assertIsNotNone(msi.find(SIGNPATH_NS + "pe-file"))
        self.assertIsNotNone(msi.find(SIGNPATH_NS + "authenticode-sign"))

        sign = workflow.index("Sign Windows executable and MSI with SignPath")
        repack = workflow.index("Repack updates from the signed executable")
        verify = workflow.index("Verify Windows signatures and signed update payload")
        upload = workflow.index("Upload LunCoSim artifact")
        self.assertLess(sign, repack)
        self.assertLess(repack, verify)
        self.assertLess(verify, upload)
        self.assertIn("--pack-only --without-msi", workflow)
        self.assertIn("luncosim-signed-update-check.zip", workflow)

    def test_release_keeps_human_installers_and_machine_updates_separate(self) -> None:
        workflow = (ROOT / ".github/workflows/dev.yml").read_text(encoding="utf-8")

        self.assertIn("python3 scripts/stage_dev_release.py", workflow)
        self.assertIn("LunCoSim/lunco-sim-updates", workflow)
        self.assertIn("secrets.LUNCOSIM_UPDATES_TOKEN", workflow)
        self.assertIn("--draft", workflow)
        self.assertIn("--draft=false", workflow)
        self.assertIn('gh release upload "$RELEASE_TAG"', workflow)
        self.assertIn("--latest=false", workflow)
        self.assertIn("--latest=true", workflow)
        self.assertNotIn("--clobber", workflow)

    def test_release_notes_describe_a_signed_dev_release_without_updater_mechanics(self) -> None:
        notes_script = (ROOT / "scripts/generate_dev_release_notes.py").read_text(encoding="utf-8")
        workflow = (ROOT / ".github/workflows/dev.yml").read_text(encoding="utf-8")

        self.assertIn("Authenticode-signed", notes_script)
        self.assertIn("download new development builds", notes_script)
        self.assertIn("scripts/generate_dev_release_notes.py", workflow)
        self.assertNotIn("Velopack update", notes_script)
        self.assertNotIn("Download update", notes_script)

    def test_application_reads_the_machine_only_repository(self) -> None:
        updater = (ROOT / "crates/lunco-updater/src/lib.rs").read_text(encoding="utf-8")

        self.assertIn('"https://github.com/LunCoSim/lunco-sim-updates"', updater)
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
        self.assertIn("iconutil -c icns", package_builder)
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
        self.assertIn("desktop filename does not match its Icon", verifier)
        self.assertIn("StartupWMClass", verifier)
        self.assertIn(".DirIcon", verifier)


if __name__ == "__main__":
    unittest.main()
