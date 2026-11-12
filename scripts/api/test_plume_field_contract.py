#!/usr/bin/env python3
"""Exercise shared thermal/plume mathematics on the native GPU, with a red control."""
import argparse
import json
import os
from pathlib import Path
import time

from PIL import Image
from runtime import ProductionSession, assert_render_log_clean, port_is_open


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, default=4881)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    log = output / 'runtime.log'
    results = {}
    with ProductionSession(args.port, windowed=True, log_path=log,
            extra_args=('--scene', 'scenes/tests/plume_field_contract.usda',
                        '--no-throttle', '--render-quality', 'high')) as session:
        def execute(command, **params):
            result = session.post({'type': 'ExecuteCommand', 'command': command, 'params': params})
            if result.get('error'):
                raise RuntimeError(result)
            return result.get('data', {})

        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            audit = execute('SceneCameraAudit')
            if any(row['usd_path'] == '/PlumeFieldContract/Camera' and row['camera_active']
                   for row in audit['candidates']):
                break
            time.sleep(.1)
        else:
            raise RuntimeError(f'contract camera not active: {audit}')
        panel = int(execute('RunRhai', code='print(find_path("/PlumeFieldContract/Panel"));')['stdout'].strip())
        for name, value in [('positive', 0.), ('negative', 1.), ('restored', 0.)]:
            if name != 'positive':
                execute('SetObjectProperty', entity_id=panel, property='failure_control', value=str(value))
            path = output / f'{name}.png'
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                time.sleep(.5)
                session.capture_screenshot(path)
                with Image.open(path) as image:
                    width, height = image.size
                    pixels = image.convert('RGB').crop((width*.4, height*.4, width*.6, height*.6))
                    # AgX desaturates primaries; compare channel dominance.
                    red = sum(r > 100 and r > g+50 and r > b+50 for r, g, b in pixels.getdata())
                    green = sum(g > 100 and g > r+50 and g > b+50 for r, g, b in pixels.getdata())
                assert_render_log_clean(log)
                if green and ((value == 0. and red == 0) or (value > 0. and red > 0)):
                    break
            results[name] = {'red': red, 'green': green}
            if green == 0 or (red == 0) != (value == 0.):
                raise RuntimeError(f'GPU field contract {name} failed: {results[name]}')
            assert_render_log_clean(log)
        ready = execute('GetReadiness')
        if not ready['ready'] or ready['world_hold'] or ready['faulted']:
            raise RuntimeError(ready)
    result = {'pixels': results, 'camera': audit, 'readiness': ready,
              'port_released': not port_is_open(args.port)}
    (output / 'validation.json').write_text(json.dumps(result, indent=2))
    assert result['port_released']
    print('PASS native thermal/plume field contract, red negative control, restored green, owned API Exit')


if __name__ == '__main__':
    main()
