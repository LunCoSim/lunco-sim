import json
from pathlib import Path
import tempfile
import unittest

from private_wayland import output_repaints


class OutputRepaintTest(unittest.TestCase):
    def test_only_complete_posts_and_finishes_in_measurement_window(self):
        def event(kind, timestamp_ms):
            return (json.dumps({"N": "core_repaint_" + kind, "wo": 1,
                                "vblank_monotonic": [0, timestamp_ms * 1_000_000]}) + "\n").encode()

        first_post = event("posted", 0)
        records = [first_post, event("finished", 5),
                   event("posted", 10), event("finished", 15),
                   event("posted", 30), event("finished", 35),
                   event("posted", 50), event("finished", 55)]
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "weston.log"
            log.write_bytes(b"".join(records))
            # The first post begins before the window. The last completion
            # straddles its end. Only the two middle repaints qualify.
            end = log.stat().st_size - 3
            result = output_repaints(log, len(first_post) // 2, end)
            self.assertEqual(result["count"], 2)
            self.assertEqual(result["mean_hz"], 50.0)
            self.assertEqual(result["p95_ms"], 20.0)
            with self.assertRaisesRegex(ValueError, "expected completed repaints"):
                output_repaints(log, len(first_post), len(first_post) + len(records[1]))
