#!/usr/bin/env python3
"""Assert that named entities exist in a running sandbox's scene graph.

Uses the synchronous `ListEntities` API request (served in the `Update` observer,
straight off the entity registry) — so it works even on a networked CLIENT, whose
sim is host-authoritative and where scripts (rhai/python) are deliberately NOT run
(`scripts_run_here` is false for a client, so a rhai assertion would never drain).
Each entity's `name` is its full USD prim path.

Usage:  entities_present.py <port> <prim-path> [<prim-path> ...]
Prints one line per name (PRESENT/MISSING) plus a summary; exits 0 iff all present.
"""
import json
import sys
import urllib.request


def list_entities(port):
    body = json.dumps({"type": "ListEntities"}).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}/api/commands",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    resp = json.loads(urllib.request.urlopen(req, timeout=5).read())
    data = resp.get("data") or {}
    ents = data.get("entities") if isinstance(data, dict) else data
    ents = ents or []
    return {e.get("name") for e in ents if isinstance(e, dict) and e.get("name")}


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    port = int(sys.argv[1])
    wanted = sys.argv[2:]
    try:
        names = list_entities(port)
    except Exception as e:  # noqa: BLE001 - report any transport failure as a miss
        print(f"ERROR: ListEntities failed on :{port}: {e}")
        sys.exit(2)
    ok = True
    for w in wanted:
        present = w in names
        ok = ok and present
        print(f"  {'PRESENT' if present else 'MISSING'}  {w}")
    print(f"ENTITIES_TOTAL={len(names)}")
    print("ENTITIES_OK" if ok else "ENTITIES_MISSING")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
