#!/usr/bin/env python3
"""Check camera-dependent native shadow geometry with a moving Sun."""
from __future__ import annotations

import argparse
import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open
from run_editor_scene_test import wait_for_scene, wait_for_scene_time_selection


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4784)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_CONFIG=str(output / 'config'),
                      LUNCOSIM_EPHEMERAL_SETTINGS='1', LUNCOSIM_ISOLATED_RUN='1')
    scene = ROOT / 'assets/scenes/tests/vertex_shadow_alignment.usda'
    log = output / 'runtime.log'
    rows = []
    with ProductionSession(args.port, windowed=True, log_path=log, extra_args=[
        '--scene', str(scene), '--render-quality', 'high', '--no-throttle', '--no-vsync',
    ]) as session:
        pid = session.process.pid
        if sys.platform.startswith('linux') and (
            Path(f'/proc/{pid}/cwd').resolve() != ROOT
            or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve()
        ):
            raise RuntimeError('session does not match this checkout and production binary')

        def execute(command, **params):
            result = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if result.get('error'):
                raise RuntimeError(f'{command}: {result}')
            return result.get('data', {})

        wait_for_scene(session, str(scene), 30)
        wait_for_scene_time_selection(session, log, 30)
        execute('SetTimeTransport', playing=True)
        cameras = execute('SceneCameraAudit')['candidates']
        if not any(c['camera_active'] for c in cameras) or not any(not c['camera_active'] for c in cameras):
            raise RuntimeError('fixture needs both an active camera and an inactive camera')
        time.sleep(2)
        for index in range(12):
            # Large, alternating rotations exercise new cascade transforms at
            # each sample while preserving visible, nonoverlapping probe areas.
            yaw = .35 * math.sin(index * 1.7)
            pitch = -.8 + .18 * math.cos(index * 1.3)
            execute('SetEnvironmentLight', sun_yaw=yaw, sun_pitch=pitch)
            time.sleep(.15)
            path = output / f'phase-{index:02}.png'
            session.capture_screenshot(path)
            colors = {}
            for name, expression in {
                'green': 'g > 0.2 && g > 1.5*r && g > 1.5*b',
                'cyan': 'g > 0.2 && b > 0.2 && g > 1.5*r && b > 1.5*r',
                'red': 'r > 0.2 && r > 1.5*g && r > 1.5*b',
            }.items():
                colors[name] = float(subprocess.check_output([
                    'convert', str(path), '-fx', expression, '-format', '%[fx:mean]', 'info:'
                ], text=True))
            passed = colors['green'] > .0001 and colors['cyan'] > .0001 and colors['red'] < .00001
            rows.append(dict(index=index, yaw=yaw, pitch=pitch, pixels=colors, passed=passed))
            print(json.dumps(rows[-1]), flush=True)
        diagnostics = execute('RuntimeDiagnostics')
    assert_render_log_clean(log)
    if 'VERTEX PREPASS VARIANTS: PASS' not in log.read_text():
        raise RuntimeError('authored native camera prepass checks did not pass')
    assert not port_is_open(args.port)
    result = dict(phases=rows, diagnostics=diagnostics, pid=pid, port_released=True)
    (output / 'validation.json').write_text(json.dumps(result, indent=2))
    if diagnostics['errors'] or any(not row['passed'] for row in rows):
        raise RuntimeError('native shadow geometry disagrees with the visible caster')
    print('PASS twelve moving-Sun GPU phases; owned port released', flush=True)


if __name__ == '__main__':
    main()
