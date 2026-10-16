#!/usr/bin/env python3
"""Verify resource provenance after native file and directory renames."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import time

from runtime import BINARY, ROOT, ProductionSession, assert_render_log_clean, port_is_open
from test_asset_resources import execute, verdict, wait_document

PORT = int(os.environ.get("LUNCOSIM_API_PORT", "4286"))
OUTPUT = Path(os.environ.get("LUNCOSIM_RESOURCE_TEST_OUTPUT", ROOT / ".lunco/asset-resource-renames")).resolve()


def main():
    image = Path(os.environ["LUNCOSIM_RESOURCE_TEST_IMAGE"]).resolve(strict=True)
    OUTPUT.mkdir(parents=True, exist_ok=True)
    twin = OUTPUT / "rename-twin"
    original = twin / "models # %"
    moved = twin / "moved # %"
    final = twin / "final # %"
    resource = original / "Resources/plot.png"
    resource.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(image, resource)
    source = original / "RelocationProbe.mo"
    source.write_text("within; package RelocationProbe end RelocationProbe;\n")
    (twin / "twin.toml").write_text('name = "resource_rename_probe"\nversion = "0.1.0"\n')
    os.environ.update(LUNCOSIM_CONFIG=str(OUTPUT / "config"),
                      LUNCOSIM_EPHEMERAL_SETTINGS="1", LUNCOSIM_ISOLATED_RUN="1")
    log = OUTPUT / "production.log"
    captures = []
    with ProductionSession(PORT, windowed=True, log_path=log) as session:
        pid = session.process.pid
        if Path("/proc").is_dir():
            assert Path(f"/proc/{pid}/cwd").resolve() == ROOT
            assert Path(f"/proc/{pid}/exe").resolve() == BINARY.resolve()
        (OUTPUT / "session.json").write_text(json.dumps({"pid": pid, "port": PORT,
            "cwd": str(ROOT), "executable": str(BINARY.resolve())}, indent=2))

        def capture(name, uri):
            execute(session, "OpenAssetResource", uri=uri)
            time.sleep(4)
            session.capture_screenshot(OUTPUT / f"{name}.png")
            captures.append(name)
            print(f"captured {name}", flush=True)

        old_uri = "modelica://RelocationProbe/Resources/plot.png"
        new_uri = "modelica://RelocatedProbe/Resources/plot.png"
        execute(session, "OpenTwin", path=str(twin))
        execute(session, "OpenFile", path=str(source))
        doc = wait_document(session, source)
        verdict("verify_resource_rename", doc, str(source), "RelocationProbe")
        capture("before", old_uri)
        execute(session, "RenameTwinEntry", twin_root=str(twin),
                relative_path=original.name, new_name=moved.name)
        source = moved / source.name
        wait_document(session, source)
        verdict("verify_resource_rename", doc, str(source), "RelocationProbe")
        capture("folder-renamed", old_uri)
        execute(session, "CloseDocument", doc_id=doc)
        capture("folder-closed", old_uri)
        execute(session, "OpenFile", path=str(source))
        doc = wait_document(session, source)
        verdict("verify_resource_rename", doc, str(source), "RelocationProbe")
        capture("folder-reopened", old_uri)
        execute(session, "RenameTwinEntry", twin_root=str(twin),
                relative_path=str(source.relative_to(twin)), new_name="RelocatedProbe.mo")
        source = moved / "RelocatedProbe.mo"
        wait_document(session, source, "within; package RelocatedProbe end RelocatedProbe;\n")
        verdict("verify_resource_rename", doc, str(source), "RelocatedProbe")
        capture("file-renamed", new_uri)
        capture("retired-declaration", old_uri)
        execute(session, "CloseDocument", doc_id=doc)
        capture("file-closed", new_uri)
        # A directory move also updates facts for sources without open views.
        execute(session, "RenameTwinEntry", twin_root=str(twin),
                relative_path=moved.name, new_name=final.name)
        capture("closed-folder-renamed", new_uri)
        source = final / source.name
        execute(session, "OpenFile", path=str(source))
        doc = wait_document(session, source)
        verdict("verify_resource_rename", doc, str(source), "RelocatedProbe")
        capture("file-reopened", new_uri)
        execute(session, "CloseDocument", doc_id=doc)
        # A missing member remains a diagnostic after positive rename recovery.
        capture("missing-resource", "modelica://RelocatedProbe/Resources/missing.png")
        assert_render_log_clean(log)
    assert not port_is_open(PORT)
    (OUTPUT / "validation.json").write_text(json.dumps({"port": PORT, "exit": 0,
        "port_released": True, "rhai_assertions": 20, "captures": captures}, indent=2))
    print("resource rename checks completed; inspect captures", flush=True)


if __name__ == "__main__":
    main()
