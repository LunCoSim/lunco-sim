#!/usr/bin/env python3
"""Exercise quality changes across real terrain scene replacements."""
from __future__ import annotations

import argparse
import os
import time
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open, run_authored_scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scene', required=True)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    os.environ['LUNCOSIM_CONFIG'] = str(output / 'config')
    os.environ['LUNCOSIM_EPHEMERAL_SETTINGS'] = '1'
    log = output / 'runtime.log'
    with ProductionSession(args.port, windowed=True, log_path=log,
                           extra_args=['--scene', args.scene, '--no-throttle']) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime does not match this checkout and executable')
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            response = session.post({'type': 'ExecuteCommand', 'command': 'TerrainLodStatus', 'params': {}})
            if response.get('error'):
                raise RuntimeError(response)
            status = response.get('data', {})
            contacts = status.get('contacts', [])
            stream = status.get('stream', {})
            if (contacts and all(t['ready'] and t['drawn'] and t['pinned'] for t in contacts)
                    and stream.get('wanted', 0) > 0
                    and stream['wanted'] == stream['resident'] and stream['pending'] == 0):
                break
            time.sleep(.25)
        else:
            raise RuntimeError('initial terrain cover did not settle')
        run_authored_scenario(session, 'lunco://scenarios/tests/terrain_restart_quality.rhai',
                              title='TERRAIN RESTART QUALITY', check_count=6, timeout_s=210)
        diagnostics = session.post({'type': 'ExecuteCommand', 'command': 'RuntimeDiagnostics', 'params': {}})
        if diagnostics.get('error') or diagnostics['data']['errors'] or diagnostics['data']['warnings']:
            raise RuntimeError(f'runtime diagnostics: {diagnostics}')
        assert_render_log_clean(log)
    if port_is_open(args.port):
        raise RuntimeError('owned API port was not released')
    print('PASS terrain restart with quality changes and owned Exit', flush=True)


if __name__ == '__main__':
    main()
