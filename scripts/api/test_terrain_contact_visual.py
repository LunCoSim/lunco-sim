#!/usr/bin/env python3
"""Rendered contact, route, trail, and bounded-cover regression on a real Twin."""
from __future__ import annotations
import argparse
import json
import os
import math
import time
from pathlib import Path
from runtime import BINARY, ProductionSession, ROOT, assert_render_log_clean, run_authored_scenario, port_is_open


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--scene', required=True)
    p.add_argument('--body-path', required=True)
    p.add_argument('--route-path', required=True)
    p.add_argument('--port', type=int, default=4794)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--no-vsync', action='store_true', help='diagnostic uncapped presentation')
    p.add_argument('--no-throttle', action='store_true', help='diagnostic continuous host updates')
    args = p.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(out / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    rows = []
    log = out / 'runtime.log'
    extra_args = ['--scene', args.scene, '--window-pos', '1280x720+0+0']
    if args.no_vsync:
        extra_args.append('--no-vsync')
    if args.no_throttle:
        extra_args.append('--no-throttle')
    with ProductionSession(args.port, windowed=True, log_path=log,
            extra_args=extra_args) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT:
            raise RuntimeError('owned runtime cwd does not match this checkout')
        if Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime executable does not match LUNCOSIM_BIN')
        cohort = {'pid': pid, 'binary': str(BINARY.resolve()), 'no_vsync': args.no_vsync,
                  'no_throttle': args.no_throttle, 'phases': rows}
        def execute(command, **params):
            response = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if response.get('error'):
                raise RuntimeError(response)
            return response.get('data', response)
        def rhai(code):
            return execute('RunRhai', code=code)
        def settle():
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                s = execute('TerrainLodStatus')
                stream = s['stream']
                if (s['contacts'] and all(t['ready'] and t['drawn'] and t['pinned'] for t in s['contacts'])
                        and stream['wanted'] == stream['resident'] and stream['pending'] == 0):
                    return s
                time.sleep(.15)
            raise RuntimeError(f"contact cover did not settle: {s['stream']}; contacts={s['contacts']}; cameras={s['camera_cover']}")
        def route_view():
            deadline = time.monotonic() + 30
            route_path = json.dumps(args.route_path)
            while time.monotonic() < deadline:
                doc_id = execute('QueryUsdPrim', path=args.route_path)['doc_id']
                code = f'let d=query("QueryUsdPrim", #{{path:{route_path}}}).doc_id; print(find_path_in_document(d,waypoint_editor::ribbon_geometry_path({route_path})));'
                curve_id = int(rhai(code)['stdout'].strip())
                if curve_id > 0:
                    view = execute('InspectUsdCurveView', doc_id=doc_id, entity_id=curve_id)
                    if view['state'] == 'failed':
                        raise RuntimeError(view)
                    if view['state'] == 'ready' and view['displayed_surface_binding_count'] > 0:
                        return doc_id, curve_id
                time.sleep(.15)
            raise RuntimeError('route projection/annotation publication did not become ready')
        def phase(name, trails):
            status = settle()
            doc_id, curve_id = route_view()
            run_authored_scenario(session, 'lunco://scenarios/tests/terrain_contact_visual.rhai',
                title='TERRAIN CONTACT VISUAL', check_count=11,
                params={'body_path': args.body_path, 'route_path': args.route_path,
                        'require_trails': trails, 'doc_id': doc_id, 'curve_id': curve_id, 'require_finer': name == 'close-camera'}, timeout_s=90)
            time.sleep(.3)
            session.capture_screenshot(out / f'{name}.png')
            rows.append({'phase': name, 'config': status['config'], 'stream': status['stream'],
                         'contacts': status['contacts'], 'camera_cover': status['camera_cover'], 'performance': execute('EnginePerformance')})
            (out / 'validation.json').write_text(json.dumps(cohort, indent=2))
            assert_render_log_clean(log)
        phase('initial', False)
        body = json.dumps(args.body_path)
        body_id = int(rhai(f'print(find_path({body}));')['stdout'].strip())
        start_position = execute('QueryEntity', id=body_id)['position']
        execute('AcquireControl', target=body_id, bind_camera=False)
        execute('SetPorts', target=body_id, producer_id=4794001, seq=0, tick=0,
                writes=[['program_active', 1.0], ['autopilot_enable', 0.0],
                        ['throttle', .35], ['steer', 0.0], ['brake', 0.0]])
        # Observe the production drive with the selected host flags and unchanged solver.
        frame_samples = {}
        first_frame = execute('EnginePerformance')['frame_sample_count']
        drive_deadline = time.monotonic() + 15
        while time.monotonic() < drive_deadline:
            performance = execute('EnginePerformance')
            count = performance['frame_sample_count']
            history = performance['frame_time_samples_ms']
            for i, value in enumerate(history):
                sample_id = count-len(history)+i+1
                if sample_id > first_frame:
                    frame_samples[sample_id] = value
            time.sleep(1)
        position = execute('QueryEntity', id=body_id)['position']
        distance = math.dist(start_position, position)
        if distance < 2:
            raise RuntimeError(f'drive did not exercise tile boundaries: {distance} m')
        ordered = sorted(frame_samples.values())
        drive_metrics = {'distance_m': distance, 'sample_count': len(ordered),
            'first_sample_id': min(frame_samples), 'last_sample_id': max(frame_samples),
            'frame_ms_p50': ordered[len(ordered)//2],
            'frame_ms_p95': ordered[int(.95*(len(ordered)-1))],
            'frame_ms_max': ordered[-1]}
        rows.append({'phase': 'drive-performance', **drive_metrics})
        print(json.dumps(drive_metrics), flush=True)
        session.capture_screenshot(out / 'driving.png')
        execute('SetPorts', target=body_id, producer_id=4794001, seq=0, tick=0,
                writes=[['throttle', 0.0], ['steer', 0.0], ['brake', 1.0]])
        time.sleep(2)
        phase('post-drive', True)
        # A lower discretionary budget/depth must not erase physical ground detail.
        execute('SetTerrainRenderingQuality', max_depth=6, tile_resolution=33, tile_budget=64)
        phase('constrained-budget', True)
        execute('SetTerrainRenderingQuality', max_depth=10, tile_resolution=65, tile_budget=256)
        phase('finer-camera', True)
        execute('SetUserCamera', name='/Traverse/Avatar')
        execute('FollowTarget', target=body_id)
        execute('SetCameraFollow', distance_m=150.0, height_m=80.0)
        time.sleep(1)
        phase('far-camera', True)
        execute('SetCameraFollow', distance_m=2.8, height_m=1.4)
        time.sleep(1)
        phase('close-camera', True)
        diagnostics = execute('RuntimeDiagnostics')
        if diagnostics['errors'] or diagnostics['warnings']:
            raise RuntimeError(diagnostics)
    assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError('owned API port was not released')
    cohort.update(diagnostics=diagnostics, port_released=True)
    (out / 'validation.json').write_text(json.dumps(cohort, indent=2))
    print('PASS contact geometry, route/trail bindings, drive, constrained graphics, owned Exit', flush=True)

if __name__ == '__main__':
    main()
