#!/usr/bin/env python3
"""Capture paired production profiles into a candidate exact-state reference.

The maintained reference supplies the profile matrix. Each process keeps every
assertion enabled; only a mismatch against that reference is allowed during
capture. This command writes a candidate and never replaces the maintained file.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / 'api'))
from runtime import BINARY, ROOT, ProductionSession

PREFIX = 'LUNCO_SCENE_TEST_EVIDENCE '
COMPARISON_CHECK = 'physics, Modelica, and articulated states exactly match the selected reference; '


def value(raw):
    """Decode the transport-owned TelemetryValue envelope once at the wire."""
    if not isinstance(raw, dict) or len(raw) != 1:
        raise ValueError('malformed typed telemetry envelope')
    kind, body = next(iter(raw.items()))
    if kind == 'Map':
        return {key: value(item) for key, item in body.items()}
    if kind == 'Array':
        return [value(item) for item in body]
    if kind in ('F64', 'I64', 'U64', 'Bool', 'String'):
        return body
    raise ValueError(f'unknown typed telemetry kind {kind}')


def capture(profile, expected, reference, port, log, require_pass=False):
    parameters = expected['parameters']
    threads = {'single': 1, 'default': 0}[parameters['thread_setting']]
    arguments = ['--scene', parameters['scene'], '--threads', str(threads),
                 '--jitter', str(parameters['jitter']), '--seed', str(parameters['seed']),
                 '--tick-hz', str(parameters['tick_hz']), '--determinism-reference', str(reference)]
    os.environ.update(LUNCOSIM_EPHEMERAL_SETTINGS='1', LUNCOSIM_ISOLATED_RUN='1',
                      LUNCOSIM_CONFIG=str(log) + '.config')
    with ProductionSession(port, subcommand='test', require_readiness=False,
                           extra_args=arguments, log_path=log) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('the launched runtime does not match this checkout and binary')
        print(f'OWNED profile={profile} pid={pid} port={port} log={log}', flush=True)
        code = session.process.wait(timeout=900)
    if code not in (0, 1):
        raise RuntimeError(f'{profile} has no normal terminal verdict (exit={code}); {log}')
    events = [json.loads(line[len(PREFIX):]) for line in log.read_text().splitlines() if line.startswith(PREFIX)]
    records = []
    metadata = []
    verdicts = []
    for event in events:
        if not event['name'].startswith('DETERMINISM_'):
            continue
        data = value(event['data'])
        if data['profile'] != profile:
            raise RuntimeError(f'event belongs to another profile: {data["profile"]}')
        if event['name'] == 'DETERMINISM_STATE_EVIDENCE': records.append(data)
        elif event['name'] == 'DETERMINISM_PROFILE_EVIDENCE': metadata.append(data)
        elif event['name'] == 'DETERMINISM_VERDICT_EVIDENCE': verdicts.append(data)
    if len(metadata) != 1 or len(verdicts) != 1:
        raise RuntimeError(f'{profile} did not produce one complete metadata and verdict envelope; {log}')
    failures = [check for check in verdicts[0]['checks'] if check]
    if any(not check.startswith(COMPARISON_CHECK) for check in failures):
        raise RuntimeError(f'{profile} failed a behavioral assertion: {failures}; {log}')
    if require_pass and failures:
        raise RuntimeError(f'{profile} replay did not pass: {failures}; {log}')
    if (code == 0) != (not failures):
        raise RuntimeError(f'{profile} process exit disagrees with its authored checks; {log}')
    meta = metadata[0]
    if meta['parameters'] != parameters or meta['first_behavior_tick'] != '1':
        raise RuntimeError(f'{profile} capture metadata disagrees with its requested inputs')
    by_key = {}
    for record in records:
        key = record['kind'], record['tick'], record['identity']
        if key in by_key:
            raise RuntimeError(f'duplicate runtime capture {key}')
        by_key[key] = record['value']
    result = {key: meta[key] for key in ('parameters', 'effective_compute_width', 'first_behavior_tick')}
    roster = parameters['rover_count']
    consumed = set()

    def rows(kind, tick, shape):
        entries = sorted((identity, item) for (record_kind, record_tick, identity), item in by_key.items()
                         if record_kind == kind and record_tick == tick)
        if len(entries) != roster:
            raise RuntimeError(f'{profile}: {kind} at tick {tick} has {len(entries)} rows, expected {roster}')
        consumed.update((kind, tick, identity) for identity, _ in entries)
        if shape == 'physics': return [item for _, item in entries]
        if shape == 'modelica': return [{'lane_x': identity, 'variables': ';'.join(item)} for identity, item in entries]
        return [{'rover_path': identity, 'bodies': item} for identity, item in entries]

    for key, kind, shape, column in (
        ('physics_checkpoints', 'physics', 'physics', 'physics'),
        ('modelica_checkpoints', 'modelica', 'modelica', 'systems'),
        ('articulated_checkpoints', 'articulated', 'articulated', 'rovers'),
    ):
        result[key] = [{'tick': checkpoint['tick'], column: rows(kind, checkpoint['tick'], shape)}
                       for checkpoint in expected[key]]
    final_tick = meta['final_tick']
    if final_tick != expected['final']['tick']:
        raise RuntimeError(f'{profile} final tick changed from {expected["final"]["tick"]} to {final_tick}')
    result['final'] = {'tick': final_tick,
                       'physics': rows('final_physics', final_tick, 'physics'),
                       'modelica': rows('final_modelica', final_tick, 'modelica'),
                       'articulated': rows('final_articulated', final_tick, 'articulated')}
    if consumed != set(by_key):
        raise RuntimeError(f'{profile} emitted records outside the declared capture checkpoints')
    return result


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference', type=Path, default=ROOT/'scripts/tests/fixtures/deterministic-physics-reference.json')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--logs', type=Path, required=True)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--profile', action='append')
    parser.add_argument('--verify-only', action='store_true', help='Replay each profile once and require every authored check to pass')
    args = parser.parse_args()
    if not BINARY.is_file(): parser.error('build the production LUNCOSIM_BIN before capture')
    if args.reference.resolve() == args.output.resolve(): parser.error('candidate output must differ from the maintained reference')
    baseline = json.loads(args.reference.read_text())
    profiles = args.profile or list(baseline['profiles'])
    if not profiles or len(profiles) != len(set(profiles)): parser.error('profiles must be non-empty and unique')
    args.logs.mkdir(parents=True, exist_ok=True)
    captured = {}
    receipts = {}
    for profile in profiles:
        first = capture(profile, baseline['profiles'][profile], args.reference.resolve(), args.port, args.logs/f'{profile}.first.log', args.verify_only)
        if args.verify_only:
            if first != baseline['profiles'][profile]:
                raise RuntimeError(f'{profile} replay capture differs from the maintained reference')
            receipts[profile] = {'first': digest(args.logs/f'{profile}.first.log')}
            print(f'VERIFIED EXACT {profile}', flush=True)
        else:
            second = capture(profile, baseline['profiles'][profile], args.reference.resolve(), args.port, args.logs/f'{profile}.second.log')
            if first != second:
                (args.logs/f'{profile}.first.json').write_text(json.dumps(first, indent=2))
                (args.logs/f'{profile}.second.json').write_text(json.dumps(second, indent=2))
                raise RuntimeError(f'{profile} paired runtime states differ; captures saved beside the logs')
            receipts[profile] = {suffix: digest(args.logs/f'{profile}.{suffix}.log') for suffix in ('first', 'second')}
            print(f'PAIRED EXACT {profile}', flush=True)
        captured[profile] = first
    status = subprocess.check_output(['git', 'ls-files', '--modified', '--others', '--exclude-standard'], cwd=ROOT, text=True).splitlines()
    inputs = {name: digest(ROOT/name) for name in sorted(set(status + ['Cargo.lock']))
              if (ROOT/name).is_file() and (name.endswith(('.rs', '.mo', '.rhai', '.usda', '.usdc', '.py', 'Cargo.toml')) or name == 'Cargo.lock')}
    candidate = {key: baseline[key] for key in ('schema', 'comparison')}
    candidate.update(recorded_at_utc=datetime.now(timezone.utc).isoformat(), profiles=captured,
                     source={'capture_checkout_revision': subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
                             'binary': {'name': BINARY.name, 'sha256': digest(BINARY)},
                             'input_sha256': inputs, 'paired_log_sha256': receipts,
                             'capture_instrumentation': 'Production Rhai typed evidence through the bounded scene-test evidence wire.',
                             'validation': ('Every profile passes its authored verdict and exactly reproduces the maintained reference.' if args.verify_only else 'Every selected profile matches exactly in two independent production processes; all non-reference assertions and the altered-row discriminator remain enabled.')})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(candidate, indent=2, sort_keys=True)+'\n')
    print(f'PASS candidate={args.output}; maintained reference unchanged', flush=True)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, ValueError, KeyError) as error:
        print(f'FAIL {error}', file=sys.stderr)
        raise SystemExit(1)
