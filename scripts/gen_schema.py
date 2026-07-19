#!/usr/bin/env python3
"""Regenerate crates/lunco-usd/schema/generatedSchema.usda from schema.usda.

The schema library is CODELESS (`skipCodeGeneration = true`): the "generated"
file is not C++ codegen, it is the registered layer — `plugInfo.json` points a
USD runtime at it, and `lunco_usd::schema::SchemaRegistry` parses it at runtime.
Pixar's `usdGenSchema` produces it from `schema.usda`; this script is the
in-repo equivalent so the two files can never drift by hand-sync (the
documented trap: schema.usda is INERT, the generated file is what loads).

Transform (what usdGenSchema does that matters to a codeless consumer):
  - drop the core `subLayers` (usd/usdGeom/usdPhysics/... sources are vendored
    separately under `schema/core/` and ingested on their own)
  - drop the `inherits = </APISchemaBase>` / `</Typed>` arcs (the registry and
    codeless registration read flat class definitions)
  - keep everything else verbatim: class prims, `customData` (apiSchemaType,
    UI hints, lunco:unit), attribute types/defaults/variability, docs, comments

Usage:  python3 scripts/gen_schema.py   (from the workspace root)
"""

import re
import sys
from pathlib import Path

SCHEMA_DIR = Path("crates/lunco-usd/schema")
SRC = SCHEMA_DIR / "schema.usda"
OUT = SCHEMA_DIR / "generatedSchema.usda"

GENERATED_HEADER = '''#usda 1.0
(
    "Generated schema for luncoSchema"
    """
    GENERATED — do not edit. Regenerate with `python3 scripts/gen_schema.py`
    after editing `schema.usda`, which is the authoritative source.

    This is the file a USD runtime actually registers (via `plugInfo.json`), and
    the file `lunco_usd::schema` parses to answer "what does this property's
    schema declare?" — its type, its variability, its UI hint, and whether it is
    `custom`.
    """
)
'''


def main() -> int:
    src = SRC.read_text()

    # 1. Replace the layer-metadata header (first parenthesised block after
    #    `#usda 1.0`) with the GENERATED header. The source header carries the
    #    authoring guide + core subLayers; neither belongs in the registered
    #    layer.
    m = re.match(r"#usda 1\.0\n\(\n.*?\n\)\n", src, flags=re.DOTALL)
    if not m:
        print("error: could not find the layer metadata block in schema.usda")
        return 1
    body = src[m.end():]

    # 2. Drop `inherits = </...>` arcs (with a trailing comma if the metadata
    #    list continues). Codeless registration reads flat class definitions.
    body = re.sub(r"[ \t]*(prepend\s+)?inherits\s*=\s*</[^>]*>,?\n", "", body)

    OUT.write_text(GENERATED_HEADER + body)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) from {SRC}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
