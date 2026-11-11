#!/usr/bin/env python3
"""Check a followed camera against terrain beyond the physics collider ring."""
from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scene', required=True)
    parser.add_argument('--body-path', required=True)
    parser.add_argument('--camera-path', required=True)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--quality', choices=['low', 'balanced', 'high'], default='balanced')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    log = output / 'runtime.log'
    with ProductionSession(args.port, windowed=True, log_path=log,
                           extra_args=['--scene', args.scene, '--no-throttle',
                                       '--render-quality', args.quality]) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime does not match this checkout and executable')

        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)

        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if 'name=SIMULATION_STARTED' in log.read_text():
                break
            time.sleep(.25)
        else:
            raise RuntimeError('scene scenario did not start')
        target = int(execute('RunRhai', code=f'print(find_path({json.dumps(args.body_path)}));')['stdout'].strip())
        if target <= 0:
            raise RuntimeError('follow target is unavailable')
        execute('SetUserCamera', name=args.camera_path)
        execute('FollowTarget', target=target)
        execute('SetCameraFollow', distance_m=80.0, height_m=32.0, yaw_rad=0.0, pitch_rad=-0.05)
        time.sleep(5)
        execute('SetTimeTransport', playing=False)
        run_authored_scenario(session, 'lunco://scenarios/tests/camera_terrain_follow.rhai',
                              title='CAMERA TERRAIN FOLLOW', check_count=8, timeout_s=90)
        diagnostics = execute('RuntimeDiagnostics')
        if diagnostics['errors'] or diagnostics['warnings']:
            raise RuntimeError(f'runtime diagnostics: {diagnostics}')
        assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError('owned API port was not released')
    print('PASS camera terrain follow with owned Exit', flush=True)


if __name__ == '__main__':
    main()
