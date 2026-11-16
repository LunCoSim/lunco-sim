#!/usr/bin/env python3
"""Check public source identities across fresh private mount leases."""
import argparse
import json
import re
from pathlib import Path
import uuid

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean
from test_scene_loading_settings import execute, wait_for


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    args = parser.parse_args()
    log = ROOT / 'target/stable-asset-references.log'
    with ProductionSession(args.port, windowed=True, log_path=log) as session:
        pid = session.process.pid
        if Path(f'/proc/{pid}/cwd').resolve() != ROOT or Path(f'/proc/{pid}/exe').resolve() != BINARY.resolve():
            raise RuntimeError('owned process does not match checkout')

        def command(command_name, **params):
            return execute(session, command_name, **params)

        def rhai(code):
            return command('RunRhai', code=code)['stdout'].strip()

        previous = None
        metadata = []
        for cycle in range(2):
            command('OpenTwin', path=str(ROOT / 'assets/scenes/tests/twin_search_paths'))
            twin = wait_for(session, lambda: (value if (value := command('InspectWorkbenchNavigation').get('twin_id')) not in (None, previous) else None), 'fresh Twin')
            previous = twin
            command('OpenTwinScene', twin_id=twin, relative_path='sim/scenes/search_paths.usda')
            target = wait_for(session, lambda: (value if (value := int(rhai('print(find("/World/SearchRootProbe"));'))) >= 0 else None), 'projected program')
            wait_for(session, lambda: command('GetReadiness')['ready'] and not command('GetReadiness')['world_hold'], 'scene readiness')
            for preview in command('InspectUsdViewport')['previews']:
                command('CloseUsdPreview', preview=preview['preview'])
            command('ActivatePerspective', id='rover_build')
            command('FocusPanel', id='rhai_editor')
            command('SelectEntity', entity_id=target)
            doc = wait_for(session, lambda: command('InspectBehaviourEditor')['selected_document'], 'script document')
            # Direct HTTP is intentional: the scripting adapter also normalizes
            # diagnostics, so it cannot prove the provider's wire boundary.
            item = command('InspectScriptDocument', doc_id=doc)
            expected = 'twin://twin_search_paths/scripts/search_root_probe.rhai'
            if item['asset_id'] != expected:
                raise RuntimeError(f"incorrect public script source: {item['asset_id']}")
            if item['origin']['uri'].startswith('twin://') and item['origin']['uri'] != expected:
                raise RuntimeError(f"private script origin leaked: {item['origin']['uri']}")
            metadata.append({'doc_id': doc, 'asset_id': item['asset_id']})
            print(f'PASS: remount {cycle + 1} public script source = {expected}', flush=True)
            command('ActivatePerspective', id='editor')
            command('OpenSourceView', asset_path=expected)
            wait_for(session, lambda: command('InspectWorkbenchNavigation')['source'] == expected, 'stable source tab')
            print(f'PASS: remount {cycle + 1} source tab uses stable identity', flush=True)
        if metadata[0]['doc_id'] == metadata[1]['doc_id']:
            raise RuntimeError('script document lease survived Twin retirement')

        engine_source = 'lunco://scripting/tests/test_ui_review.rhai'
        command('OpenSourceView', asset_path=engine_source)
        wait_for(session, lambda: command('InspectWorkbenchNavigation')['source'] == engine_source, 'engine source URI')
        print('PASS: engine URI resolves through the same source viewer', flush=True)

        catalog_rows = []
        for cycle in range(2):
            old = previous
            command('OpenFolder', path=str(ROOT / 'assets/scenes/tests/editor/referenced_part_edit'))
            previous = wait_for(session, lambda: (value if (value := command('InspectWorkbenchNavigation').get('twin_id')) not in (None, old) else None), 'folder remount')
            identity = rhai('print(twin_source_identity());')
            expected = f'twin://{identity}/panel.usda'
            row = wait_for(session, lambda: next((row for row in command('ListSpawnCatalog')['entries'] if row['source']['path'] == expected), None), 'stable spawn entry')
            catalog_rows.append((row['entry_id'], row['source']['path']))
            print(f'PASS: remount {cycle + 1} stable catalog entry = {catalog_rows[-1]}', flush=True)
        if catalog_rows[0] != catalog_rows[1]:
            raise RuntimeError('catalog identity changed across private mount retirement')
        shader_twin = ROOT / 'target' / ('stable-shader-' + uuid.uuid4().hex)
        old = previous
        command('CreateTwin', path=str(shader_twin), name='Stable shader fixture')
        wait_for(session, lambda: command('InspectWorkbenchNavigation').get('twin_id') not in (None, old), 'shader Twin')
        identity = rhai('print(twin_source_identity());')
        source_uri = f'twin://{identity}/shaders/stable_probe.wesl'
        command('CreateShader', name='stable_probe', template='solid')
        shader = shader_twin / 'shaders/stable_probe.wesl'
        wait_for(session, shader.exists, 'created shader')
        result = command('SetShaderSource', path=source_uri, source=shader.read_text() + '\n// stable source edit\n')
        if result.get('requested') != source_uri or result.get('queued_paths') != [source_uri]:
            raise RuntimeError(f'shader source API leaked a private load address: {result}')
        command('DeleteShader', path=source_uri)
        if shader.exists():
            raise RuntimeError('stable source deletion did not remove the shader file')
        print('PASS: shader edit response and deletion use stable UUID source identity', flush=True)
        (ROOT / 'target/stable-asset-references.json').write_text(json.dumps({'scripts': metadata, 'catalog': catalog_rows}, indent=2))
        assert_render_log_clean(log)
        if re.search(r"twin://[0-9a-f]{32}(?:/|[^0-9a-f])", log.read_text()):
            raise RuntimeError("private mount identifier leaked to the runtime log")
    print('PASS: owned process exited and port released', flush=True)


if __name__ == '__main__':
    main()
