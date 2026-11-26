from pathlib import Path
import re
import tempfile
import unittest
import zipfile

from scripts.verify_windows_asset_bundle import (
    expected_files,
    verify_installed_assets,
    verify_update,
)


ROOT = Path(__file__).parents[2]
EXPECTED_FONT_TARGETS = [
    "lunica-native",
    "luncosim-native",
    "lunica-web",
    "luncosim-web",
]


class RuntimeFontBundleContractTests(unittest.TestCase):
    def test_dejavu_is_declared_for_every_application_build(self) -> None:
        manifest = (ROOT / "assets/manifests/fonts.toml").read_text(encoding="utf-8")
        section = re.search(r"(?ms)^\[dejavu_sans\]\s*(.*?)(?=^\[|\Z)", manifest)
        self.assertIsNotNone(section, "fonts.toml must declare the DejaVu Sans artifact")
        entry = section.group(1)
        bundle = re.search(r"(?m)^bundle\s*=\s*\[([^\]]*)\]\s*$", entry)
        self.assertIsNotNone(bundle, "DejaVu Sans must declare package bundle targets")
        targets = re.findall(r'"([^"]+)"', bundle.group(1))

        self.assertEqual(targets, EXPECTED_FONT_TARGETS)
        self.assertIn('dest = "fonts/DejaVuSans.ttf"', entry)


class WindowsBundlePayloadTests(unittest.TestCase):
    def test_extracted_archive_proof_must_survive_both_package_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference = root / "reference"
            assets = root / "current" / "assets"
            package = root / "luncosim-win-x64-full.nupkg"
            files = {
                "fonts/DejaVuSans.ttf": b"fixture font bytes",
                "fonts/.DejaVuSans.ttf.integrity": b"fixture archive checksum",
            }
            for relative, content in files.items():
                for tree in (reference, assets):
                    path = tree / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(content)
            expected = expected_files(reference)
            self.assertEqual(set(expected), set(files))

            def write_update(payload: dict[str, bytes]) -> None:
                with zipfile.ZipFile(package, "w") as archive:
                    for relative, content in payload.items():
                        archive.writestr(f"lib/app/assets/{relative}", content)

            write_update(files)
            verify_update(package, expected)
            verify_installed_assets(assets, expected)

            marker = "fonts/.DejaVuSans.ttf.integrity"
            write_update({key: content for key, content in files.items() if key != marker})
            (assets / marker).unlink()
            with self.assertRaisesRegex(ValueError, "expected exactly one.*integrity"):
                verify_update(package, expected)
            with self.assertRaisesRegex(ValueError, "missing bundle file.*integrity"):
                verify_installed_assets(assets, expected)

            write_update({**files, marker: b"wrong checksum"})
            (assets / marker).write_bytes(b"wrong checksum")
            with self.assertRaisesRegex(ValueError, "changed bundle file.*integrity"):
                verify_update(package, expected)
            with self.assertRaisesRegex(ValueError, "changed bundle file.*integrity"):
                verify_installed_assets(assets, expected)

    def test_missing_reference_payload_is_not_a_success(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "No manifest bundle files"):
                expected_files(Path(directory))


if __name__ == "__main__":
    unittest.main()
