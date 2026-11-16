import unittest

from scripts.msi_version import (
    check_published_versions,
    next_development_version,
    windows_installer_version,
)


class MsiVersionTests(unittest.TestCase):
    def test_dev_semver_maps_to_monotonic_msi_versions(self):
        self.assertEqual(windows_installer_version("0.6.0-dev.1"), "0.6.1")
        self.assertEqual(windows_installer_version("0.6.0-dev.2"), "0.6.2")
        self.assertEqual(windows_installer_version("0.6.0"), "0.6.999")

    def test_release_allocator_uses_the_next_dev_sequence(self):
        assets = [
            "luncosim-0.6.0-dev.1-win-x64-full.nupkg",
            "luncosim-0.6.0-dev.2-win-x64-full.nupkg",
        ]
        self.assertEqual(next_development_version("0.6.0", assets), "0.6.0-dev.3")
        self.assertEqual(
            next_development_version(
                "0.6.0",
                ["luncosim-0.6.0-nightly.160.1-win-x64-full.nupkg"],
            ),
            "0.6.0-dev.1",
        )
        self.assertEqual(
            next_development_version(
                "0.6.0",
                [
                    "luncosim-0.6.0-nightly.161.1-win-x64-full.nupkg",
                    "luncosim-0.6.0-dev.1-win-x64-full.nupkg",
                ],
            ),
            "0.6.0-dev.2",
        )

    def test_publication_requires_newer_app_semver_versions(self):
        assets = ["luncosim-0.6.0-dev.1-win-x64-full.nupkg"]
        check_published_versions("0.6.0-dev.2", assets)
        for candidate in ("0.6.0-dev.1", "0.5.0-dev.2"):
            with self.subTest(candidate=candidate):
                with self.assertRaises(ValueError):
                    check_published_versions(candidate, assets)

        # MSI ProductVersion may reset independently from the app SemVer train.
        check_published_versions(
            "0.6.0-dev.2",
            [
                "luncosim-0.6.0-nightly.161.1-win-x64-full.nupkg",
                "luncosim-0.6.0-dev.1-win-x64-full.nupkg",
            ],
        )
        check_published_versions("0.6.0", assets)
        check_published_versions(
            "0.6.0-dev.2",
            [
                "luncosim-0.6.0-nightly.161.1-win-x64-full.nupkg",
                "luncosim-0.6.0-dev.1-win-x64-full.nupkg",
            ],
        )

    def test_invalid_or_exhausted_release_versions_fail(self):
        for version in (
            "0.6.0-dev.0",
            "0.6.0-dev.999",
            "0.6.0.1",
            "0.256.0",
        ):
            with self.subTest(version=version):
                with self.assertRaises(ValueError):
                    windows_installer_version(version)

        with self.assertRaisesRegex(ValueError, "sequence exhausted"):
            next_development_version(
                "0.6.0",
                ["luncosim-0.6.0-dev.998-win-x64-full.nupkg"],
            )


if __name__ == "__main__":
    unittest.main()
