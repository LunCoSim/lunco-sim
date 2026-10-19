#!/usr/bin/env python3
"""Check native opaque-depth clipping of the production plume in a visible window."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from runtime import ROOT, ProductionSession, assert_render_log_clean, port_is_open, wait_for_authored_verdict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4187)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--quality', choices=['low', 'balanced', 'high'], default='high')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    log = output / 'runtime.log'
    captures = ROOT / 'target/plume-depth-captures' / str(args.port)
    captures.mkdir(parents=True, exist_ok=True)
    results = {}
    with ProductionSession(args.port, windowed=True,
                           extra_args=('--no-throttle', '--render-quality', args.quality,
                                       '--scene', 'scenes/tests/plume_depth_occlusion.usda'),
                           log_path=log) as session:
        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(f'{command}: {response}')
            return response.get('data', {})

        def rhai(code):
            return execute('RunRhai', code=code)['stdout'].strip()

        def entity(path):
            value = int(rhai(f'print(find_path({json.dumps(path)}));'))
            if value < 0:
                raise RuntimeError(f'missing live entity {path}')
            return value

        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            audit = execute('SceneCameraAudit')
            if (any(row['usd_path'] == '/Engine/Camera' and row['camera_active']
                    and row['render_camera'] for row in audit['candidates'])
                    and execute('GetReadiness')['ready']):
                break
            time.sleep(.1)
        else:
            raise RuntimeError(f'authored viewport did not become ready: {audit}')
        # Isolate geometric occlusion from the camera's lens-bloom halo.
        execute('SetEnvironmentLight', bloom_intensity=0.)
        execute('SetTimeTransport', playing=True)
        wait_for_authored_verdict(session, title='PLUME DEPTH CONTRACT', check_count=3)

        def capture(name):
            path = captures / f'{name}.png'
            session.capture_screenshot(path)
            shutil.copy2(path, output / path.name)
            assert_render_log_clean(log)
            # Probe well inside the opaque bar silhouette, away from AA/bloom
            # edges and application chrome. Its surface contributes no red.
            return float(subprocess.check_output([
                'convert', str(path), '-gravity', 'center', '-crop', '12x12+0+0',
                '+repage', '-format', '%[fx:mean.r]', 'info:',
            ], text=True))

        for camera_name in ['Camera', 'OrthographicCamera']:
            camera_path = f'/Engine/{camera_name}'
            execute('SetUserCamera', name=camera_path)
            camera_id = entity(camera_path)
            values = {}
            for case, z, camera_z in [('behind', -2., 8.), ('embedded', 0., 8.),
                                       ('foreground', 2., 8.), ('inside', 0., .5)]:
                if rhai(f'print(set(find_path("/Engine/Occluder"),"Transform.translation.z",{z}));') != 'true':
                    raise RuntimeError('could not place the depth occluder')
                eye = [0., -2., camera_z]
                execute('SetCameraLookAt', camera=camera_id, eye=eye, target=[0., -2., 0.])
                time.sleep(.5)
                pose = execute('QueryEntity', id=camera_id)
                if max(abs(a-b) for a, b in zip(eye, pose['position'])) > 1e-4:
                    raise RuntimeError(f'camera pose changed before capture: {pose}')
                values[case] = capture(f'{camera_name}-{case}')
            full = values['behind']
            checks = {
                'unoccluded_gas_visible': full > .02,
                'foreground_blocks_all_emission': values['foreground'] < .005,
                'embedded_surface_clips_only_rear_gas': .15*full < values['embedded'] < .9*full,
                'inside_camera_retains_foreground_gas': values['inside'] > .01,
            }
            results[camera_name] = {'red': values, 'checks': checks}
            print(json.dumps({camera_name: results[camera_name]}), flush=True)
        ready = execute('GetReadiness')
        if not ready['ready'] or ready['faulted'] or ready['world_hold']:
            raise RuntimeError(f'faulted graphics fixture: {ready}')
        assert_render_log_clean(log)
    evidence = {'quality': args.quality, 'cameras': results, 'readiness': ready,
                'port_released': not port_is_open(args.port)}
    (output / 'validation.json').write_text(json.dumps(evidence, indent=2))
    if not evidence['port_released'] or any(not all(row['checks'].values()) for row in results.values()):
        raise RuntimeError(f'plume depth pixels failed; inspect {output / "validation.json"}')
    print('PASS native depth contract + eight projection/occlusion checks; owned API Exit', flush=True)


if __name__ == '__main__':
    main()
