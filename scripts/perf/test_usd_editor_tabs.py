"""Negative measurement cases that invalidate an equal-camera FPS comparison."""

import unittest

from usd_editor_tabs import require_diagnostics, require_physics_progress, require_ready, require_same_camera


class MeasurementGuardTests(unittest.TestCase):
    def test_orbit_rotation_and_viewport_resize_reject_equal_camera(self):
        camera = {"view": 1, "mode": "visual", "projection": "perspective",
                  "target": [0, 0, 0], "yaw": .4, "pitch": .2, "distance": 4,
                  "orthographic_scale": 1, "image_rect": {"origin": [0, 0], "size": [800, 600]},
                  "scale_factor": 1}
        require_same_camera(camera, dict(camera))
        for change in ({"yaw": .5}, {"pitch": .3}, {"image_rect": {"origin": [0, 0], "size": [600, 600]}}):
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                require_same_camera(camera, {**camera, **change})

    def test_early_advance_does_not_mask_later_physics_stall(self):
        initial = {"step_number": 10, "bodies": 4, "colliders": 4, "dynamic_bodies": 2, "joints": 1}
        advanced = {**initial, "step_number": 70}
        require_physics_progress(initial, advanced, "first second")
        with self.assertRaises(RuntimeError):
            require_physics_progress(advanced, advanced, "later second")
        with self.assertRaises(RuntimeError):
            require_physics_progress(advanced, {**advanced, "step_number": 130, "bodies": 5}, "topology change")

    def test_nonterminal_runtime_error_rejects_window(self):
        require_diagnostics({"errors": 0, "warnings": 1, "findings": []}, "warning")
        with self.assertRaisesRegex(RuntimeError, "telemetry-event-overflow"):
            require_diagnostics({"errors": 1, "findings": [
                {"code": "telemetry-event-overflow", "severity": "error"}]}, "held inbox")

    def test_fault_and_late_readiness_registration_reject_window(self):
        ready = {"ready": True, "world_hold": False, "faulted": False,
                 "readiness_tracked": True, "pending_count": 0}
        require_ready(ready, "settled")
        for change in ({"faulted": True}, {"world_hold": True}, {"pending_count": 1},
                       {"readiness_tracked": False}, {"ready": False}):
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                require_ready({**ready, **change}, "regressed")


if __name__ == "__main__":
    unittest.main()
