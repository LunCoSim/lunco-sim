from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "stage_nightly_release.py"
SPEC = importlib.util.spec_from_file_location("stage_nightly_release", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
stage_nightly_release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = stage_nightly_release
SPEC.loader.exec_module(stage_nightly_release)


class StageNightlyReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.raw = self.root / "raw"
        self.public = self.root / "public"
        self.updates = self.root / "updates"
        self.raw.mkdir()
        for runtime in stage_nightly_release.RUNTIMES:
            output = self.raw / runtime.channel
            output.mkdir()
            package_name = f"LunCoSim-{runtime.channel}-1.2.3-full.nupkg"
            package_bytes = f"package for {runtime.channel}".encode()
            (output / package_name).write_bytes(package_bytes)
            (output / f"generated{runtime.installer_suffix}").write_bytes(b"installer")
            (output / f"releases.{runtime.channel}.json").write_text(
                json.dumps(
                    {
                        "Assets": [
                            {
                                "PackageId": f"LunCoSim-{runtime.channel}",
                                "Version": "1.2.3",
                                "Type": "Full",
                                "FileName": package_name,
                                "SHA256": hashlib.sha256(package_bytes).hexdigest(),
                                "Size": len(package_bytes),
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            # Velopack also emits these, but neither is part of the public
            # download contract or the JSON update protocol.
            (output / f"LunCoSim-{runtime.channel}-Portable.zip").write_bytes(
                b"portable"
            )
            (output / f"RELEASES-{runtime.channel}").write_text("legacy index")

    def tearDown(self) -> None:
        self.temp.cleanup()

    def test_stages_four_human_downloads_and_runtime_update_feeds(self) -> None:
        stage_nightly_release.stage_release(self.raw, self.public, self.updates)

        self.assertEqual(
            {path.name for path in self.public.iterdir()},
            {runtime.public_name for runtime in stage_nightly_release.RUNTIMES},
        )
        self.assertEqual(len(list(self.updates.glob("releases.*.json"))), 4)
        self.assertEqual(len(list(self.updates.glob("*-full.nupkg"))), 4)
        self.assertFalse(any("Portable" in path.name for path in self.updates.iterdir()))
        self.assertFalse(
            any(path.name.startswith("RELEASES-") for path in self.updates.iterdir())
        )

    def test_rejects_feed_that_escapes_its_runtime_directory(self) -> None:
        feed = self.raw / "linux-x64" / "releases.linux-x64.json"
        document = json.loads(feed.read_text(encoding="utf-8"))
        document["Assets"][0]["FileName"] = "../wrong-package.nupkg"
        feed.write_text(json.dumps(document), encoding="utf-8")

        with self.assertRaisesRegex(stage_nightly_release.StagingError, "unsafe asset"):
            stage_nightly_release.stage_release(self.raw, self.public, self.updates)

    def test_rejects_missing_platform_installer(self) -> None:
        (self.raw / "win-x64" / "generated-Setup.exe").unlink()

        with self.assertRaisesRegex(stage_nightly_release.StagingError, "win-x64 installer"):
            stage_nightly_release.stage_release(self.raw, self.public, self.updates)


if __name__ == "__main__":
    unittest.main()
