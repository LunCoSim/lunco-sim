#!/usr/bin/env python3
"""Compare a frozen, Modelica-derived plume's ray budgets in a native window."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import statistics
import subprocess
import sys
import time

from PIL import Image, ImageChops, ImageStat

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'api'))
from runtime import BINARY, ProductionSession, assert_render_log_clean, wait_for_authored_verdict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4881)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=128)
    parser.add_argument('--maximize', action='store_true', help='use the native desktop framebuffer')
    args = parser.parse_args()
    if not 16 <= args.samples <= 512:
        parser.error('--samples must be in 16..512')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ.update(LUNCOSIM_CONFIG=str(output / 'config'), LUNCOSIM_EPHEMERAL_SETTINGS='1')
    log = output / 'runtime.log'
    report = {'binary': str(BINARY), 'binary_sha256': hashlib.sha256(BINARY.read_bytes()).hexdigest(),
              'samples': args.samples, 'maximize': args.maximize, 'views': {}, 'workloads': subprocess.check_output(
                  ['ps', '-eo', 'pid,comm,%cpu,%mem'], text=True)}
    with ProductionSession(args.port, windowed=True, log_path=output / 'authoring.log',
            extra_args=('--scene', 'scenes/tests/rocket_engine_plume_defaults.usda',
                        '--render-quality', 'high', '--no-throttle', '--no-vsync')) as session:
        def cmd(command, **params):
            result = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if result.get('error'):
                raise RuntimeError(result)
            return result.get('data', {})

        def entity(path):
            return int(cmd('RunRhai', code=f'print(find_path({json.dumps(path)}));')['stdout'].strip())

        if args.maximize:
            cmd('MaximizeWindow', maximized=True)
        wait_for_authored_verdict(session, title='PLUME CONSERVATION', check_count=52)
        engine = entity('/World/Engine')
        flame = entity('/World/Engine/Exhaust/Flame')
        cmd('SetPorts', target=engine, producer_id=args.port, writes=[['thrust_n', 1000.],
            ['propellant_flow_kgs', 1000./2600.], ['exhaust_velocity_mps', 2600.],
            ['maximum_thrust_n', 1000.]])
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if (cmd('GetPort', api_id=engine, name='thrust_n')['value'] == 1000.
                    and abs(cmd('GetPort', api_id=engine, name='propellant_flow_kgs')['value'] - 1000./2600.) < 1e-8
                    and cmd('GetPort', api_id=engine, name='radiant_power_w')['value'] > 0):
                break
            time.sleep(.05)
        else:
            raise RuntimeError('physical source did not deliver radiant power')
        cmd('SetTimeTransport', playing=False)
        time.sleep(.25)
        report['physical_source'] = {name: cmd('GetPort', api_id=engine, name=name)['value']
            for name in ['radiant_power_w', 'transported_distance_m']}
        root = Path(__file__).resolve().parents[2]
        source = root / 'assets/scenes/tests/rocket_engine_plume_defaults.usda'
        # Snapshot the actual published f32 field. This is render-test data,
        # disconnected from simulation only in the native-authored scratch copy.
        shader = (root / 'assets/shaders/plume.wesl').read_text()
        names = re.findall(r'^    (\w+): f32,', shader, re.M)
        names.remove('steps')
        field = {name: cmd('GetPort', api_id=flame, name=name)['value'] for name in names}
        assert all(value is not None for value in field.values()), field
        bounds = {name: cmd('GetPort', api_id=flame, name=name)['value']
                  for name in ['scale_x', 'scale_y', 'scale_z', 'translation_y']}
        report['frozen_field'] = field
        report['frozen_bounds'] = bounds
        cmd('OpenFile', path=str(source))
        document = next(row['doc_id'] for row in cmd('ListOpenDocuments')['open_documents']
                        if row['origin'].get('path') == str(source))
        fixture = output / 'fixture.usda'
        cmd('SaveAsDocument', doc_id=document, path=str(fixture))
        ops = [
            {'RemovePrim': {'edit_target': '@root@', 'path': '/World/Scenario'}},
            {'AddPrim': {'edit_target': '@root@', 'parent_path': '/World', 'name': 'Camera',
                         'type_name': 'Camera', 'reference': 'lunco://scenes/tests/plume_depth_occlusion.usda',
                         'reference_prim_path': '/Engine/Camera'}}]
        path = '/World/Engine/Exhaust/Flame'
        for name, value in {**field, **bounds}.items():
            ops += [{'SetConnection': {'edit_target': '@root@', 'path': path,
                      'name': 'inputs:'+name, 'type_name': 'float', 'sources': []}},
                    {'SetAttribute': {'edit_target': '@root@', 'path': '/World/Engine/Exhaust/Looks/Flame_Mat/Surface' if name in field else path,
                      'name': 'inputs:'+name, 'type_name': 'float', 'value': str(value)}}]
        for name, value in [('xformOp:scale', [bounds['scale_x'], bounds['scale_y'], bounds['scale_z']]),
                            ('xformOp:translate', [0.,bounds['translation_y'],0.])]:
            ops.append({'SetAttribute': {'edit_target': '@root@', 'path': path,
                        'name': name, 'type_name': 'double3',
                        'value': '('+','.join(map(str,value))+')'}})
        cmd('ApplyUsdOps', doc_id=document, ops=ops)
        cmd('SaveDocument', doc_id=document)
    # Open the recorded render field in a fresh owned process. Every comparison
    # verifies its published scalar values; only the integration budget varies.
    with ProductionSession(args.port, windowed=True, log_path=log,
            extra_args=('--scene', str(fixture), '--render-quality', 'high',
                        '--no-throttle', '--no-vsync')) as session:
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            flame = entity('/World/Engine/Exhaust/Flame')
            if flame >= 0 and any(row['usd_path']=='/World/Camera' for row in cmd('SceneCameraAudit')['candidates']):
                break
            time.sleep(.1)
        else:
            raise RuntimeError('scratch render field was not admitted')
        if args.maximize:
            cmd('MaximizeWindow', maximized=True)
        dimensions = None
        cmd('SetTimeTransport', playing=False)
        cmd('SetUserCamera', name='/World/Camera')
        camera = entity('/World/Camera')
        for view, eye, target in [('side', [.6,-.6,1.2], [0.,-.6,0.]),
                                  ('end', [.1,-2.4,.1], [0.,0.,0.]),
                                  ('inside', [0.,-.4,.03], [.5,-.6,0.])]:
            captures = []
            timings = []
            for label, samples in [('selected', args.samples), ('reference', 512), ('restored', args.samples)]:
                cmd('SetObjectProperty', entity_id=flame, property='steps', value=str(samples))
                time.sleep(.5)
                flame = entity('/World/Engine/Exhaust/Flame')
                camera = entity('/World/Camera')
                cmd('SetUserCamera', name='/World/Camera')
                cmd('SetCameraLookAt', camera=camera, eye=eye, target=target)
                time.sleep(1.5)
                assert cmd('GetPort', api_id=flame, name='steps')['value'] == samples
                pose = cmd('QueryEntity', id=camera)['position']
                assert max(abs(a-b) for a,b in zip(pose,eye)) < 1e-4, pose
                before = cmd('EnginePerformance')
                started = time.monotonic()
                time.sleep(3)
                after = cmd('EnginePerformance')
                elapsed = time.monotonic()-started
                count = after['frame_sample_count']-before['frame_sample_count']
                history = sorted(after['frame_time_samples_ms'][-min(count,len(after['frame_time_samples_ms'])):])
                timings.append({'label': label, 'samples': samples,
                    'fps': (after['frame_sample_count']-before['frame_sample_count'])/elapsed,
                    'median_ms': statistics.median(history),
                    'p95_ms': history[int(.95*(len(history)-1))]})
                path = output / f'{view}-{label}.png'
                session.capture_screenshot(path)
                with Image.open(path) as image:
                    if dimensions is None:
                        dimensions = image.size
                        report['size'] = dimensions
                    assert image.size == dimensions, image.size
                    w,h = image.size
                    captures.append(image.convert('RGB').crop((w*.1,h*.15,w*.9,h*.85)).copy())
                actual = {name: cmd('GetPort', api_id=flame, name=name)['value'] for name in field}
                assert actual == field, {name: [field[name], actual[name]] for name in field if actual[name] != field[name]}
                assert_render_log_clean(log)
            differences = []
            for index in [0,2]:
                delta = ImageChops.difference(captures[index], captures[1])
                stats = ImageStat.Stat(delta)
                differences.append({'rms_rgb': stats.rms, 'max_rgb': [x[1] for x in delta.getextrema()]})
            assert ImageStat.Stat(captures[1]).sum[0] > 10000, 'empty reference image'
            # Tolerances are display-space 8-bit levels, not radiometric error.
            assert all(max(d['rms_rgb']) <= .5 and max(d['max_rgb']) <= 4 for d in differences), differences
            repeat = ImageStat.Stat(ImageChops.difference(captures[0], captures[2]))
            assert max(repeat.rms) <= .25 and max(x[1] for x in ImageChops.difference(captures[0], captures[2]).getextrema()) <= 1, repeat.rms
            report['views'][view] = {'timings': timings, 'difference': differences,
                                     'repeat_rms_rgb': repeat.rms, 'camera': cmd('SceneCameraAudit')}
            print(view, json.dumps({'timings': timings, 'difference': differences}), flush=True)
            (output / 'sampling.json').write_text(json.dumps(report, indent=2))
        ready = cmd('GetReadiness')
        assert ready['ready'] and not ready['world_hold'] and not ready['faulted'], ready
        report['readiness'] = ready
    (output / 'sampling.json').write_text(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
