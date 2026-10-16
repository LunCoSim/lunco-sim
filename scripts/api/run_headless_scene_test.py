#!/usr/bin/env python3
"""Run a finite production scene test with an owned API port and bounded exit."""
from __future__ import annotations

import argparse
import subprocess
from pathlib import Path

from runtime import BINARY, ROOT, ProductionSession, RuntimeErrorWithLog


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--log', type=Path, required=True)
    parser.add_argument('--timeout', type=float, required=True)
    parser.add_argument('test_args', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if not 1 <= args.port <= 65535 or args.timeout <= 0:
        parser.error('a valid API port and positive timeout are required')
    test_args = args.test_args[1:] if args.test_args[:1] == ['--'] else args.test_args
    args.log.parent.mkdir(parents=True, exist_ok=True)
    args.log.write_text('')
    try:
        with ProductionSession(args.port, subcommand='test', require_readiness=False,
                               extra_args=test_args, log_path=args.log) as session:
            pid = session.process.pid
            if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
                raise RuntimeErrorWithLog('the owned test process does not match this checkout and binary')
            print(f'OWNED scene-test pid={pid} port={args.port} log={args.log}', flush=True)
            code = session.process.wait(timeout=args.timeout)
        return code if code >= 0 else 128 - code
    except subprocess.TimeoutExpired:
        print(f'scene-test exceeded {args.timeout:g}s; owned API Exit and process shutdown completed', flush=True)
        return 124
    except (RuntimeErrorWithLog, OSError) as error:
        with args.log.open('a') as log:
            log.write(f'\nscene-test process boundary failed: {error}\n')
        print(f'ERROR — {error}', flush=True)
        return 125


if __name__ == '__main__':
    raise SystemExit(main())
