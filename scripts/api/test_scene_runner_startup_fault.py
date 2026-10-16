#!/usr/bin/env python3
"""Require a terminal preparation fault to fail the production scene runner."""
from __future__ import annotations

import argparse
import os
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--log', type=Path, required=True)
    args = parser.parse_args()
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS='1', LUNCOSIM_ISOLATED_RUN='1',
                      LUNCOSIM_CONFIG=str(args.log.resolve()) + '.config')
    scene = 'assets/scenes/base/scene_runner_startup_fault.usda'
    with ProductionSession(args.port, subcommand='test', require_readiness=False,
                           extra_args=('--scene', scene, '--threads', '1'), log_path=args.log) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned runtime does not match the source checkout and binary')
        print(f'OWNED pid={pid} port={args.port} binary={BINARY}', flush=True)
        code = session.process.wait(timeout=450)
    output = args.log.read_text()
    if code != 1 or 'during scene preparation' not in output or 'terminal runtime fault kind=modelica-initialization-failed' not in output:
        raise RuntimeError(f'expected a preparation-fault rejection, got exit={code}; log={args.log}')
    if 'luncosim test NO-VERDICT' in output or 'panicked at' in output:
        raise RuntimeError(f'terminal preparation fault escaped its bounded diagnostic; log={args.log}')
    print(f'PASS — terminal startup fault returns exit 1 with its owner diagnostic; process and port released; log={args.log}')


if __name__ == '__main__':
    main()
