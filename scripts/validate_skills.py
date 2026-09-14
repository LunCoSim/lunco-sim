#!/usr/bin/env python3
"""Validate the repository skill catalogue without third-party packages."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path
from urllib.parse import unquote


FIELD_RE = re.compile(r"^([A-Za-z][A-Za-z0-9_-]*):(?:\s*(.*))?$")
LINK_RE = re.compile(r"(?<!!)\[[^\]]+\]\(([^)]+)\)")


def parse_frontmatter(path: Path) -> tuple[str | None, str | None]:
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0].strip() != "---":
        return None, None

    end = next((index for index, line in enumerate(lines[1:], 1) if line.strip() == "---"), None)
    if end is None:
        return None, None

    values: dict[str, list[str]] = {}
    active: str | None = None
    for line in lines[1:end]:
        match = FIELD_RE.match(line)
        if match and not line.startswith((" ", "\t")):
            key, value = match.groups()
            active = key
            values[key] = []
            if value:
                values[key].append(value.strip().strip("\"'"))
        elif active and line.startswith((" ", "\t")):
            values[active].append(line.strip())

    name = " ".join(values.get("name", [])).strip() or None
    description = " ".join(values.get("description", [])).strip() or None
    return name, description


def markdown_links(path: Path) -> list[str]:
    links: list[str] = []
    in_fence = False
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.lstrip().startswith(("```", "~~~")):
            in_fence = not in_fence
            continue
        if not in_fence:
            links.extend(match.group(1).strip() for match in LINK_RE.finditer(line))
    return links


def local_link_path(link: str) -> str | None:
    if link.startswith("<") and ">" in link:
        link = link[1 : link.index(">")]
    link = link.split("#", 1)[0].strip()
    if not link or link.startswith(("https://", "http://", "mailto:", "tel:", "#")):
        return None
    return unquote(link)


def validate(root: Path) -> list[str]:
    errors: list[str] = []
    skills_dir = root / "skills"
    readme = skills_dir / "README.md"
    skill_paths = sorted(skills_dir.glob("*/SKILL.md"))
    names: dict[str, Path] = {}

    if not readme.is_file():
        errors.append("skills/README.md is missing")

    for path in skill_paths:
        folder_name = path.parent.name
        name, description = parse_frontmatter(path)
        if not name:
            errors.append(f"{path.relative_to(root)}: missing frontmatter name")
        elif name in names:
            errors.append(
                f"{path.relative_to(root)}: duplicate skill name {name!r}; "
                f"already used by {names[name].relative_to(root)}"
            )
        else:
            names[name] = path
        if not description:
            errors.append(f"{path.relative_to(root)}: missing frontmatter description")
        if name and name != folder_name:
            errors.append(
                f"{path.relative_to(root)}: frontmatter name {name!r} "
                f"does not match directory {folder_name!r}"
            )
        if not any(line.startswith("# ") for line in path.read_text(encoding="utf-8").splitlines()):
            errors.append(f"{path.relative_to(root)}: missing Markdown title")

    manifest_path = skills_dir / "manifest.toml"
    if not manifest_path.is_file():
        errors.append("skills/manifest.toml is missing")
    else:
        try:
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as error:
            errors.append(f"skills/manifest.toml: invalid TOML: {error}")
            manifest = {}

        entries = manifest.get("skills", [])
        if not isinstance(entries, list):
            errors.append("skills/manifest.toml: skills must be an array of tables")
            entries = []
        manifest_names: list[str] = []
        required = manifest.get("contract", {}).get("required", [])
        if not isinstance(required, list) or not required:
            errors.append("skills/manifest.toml: contract.required must be non-empty")
            required = ["name", "category", "primary_for", "defer_to", "evidence"]
        for entry in entries:
            if not isinstance(entry, dict):
                errors.append("skills/manifest.toml: each skill entry must be a table")
                continue
            name = entry.get("name")
            if not isinstance(name, str) or not name:
                errors.append("skills/manifest.toml: skill entry is missing name")
                continue
            if name in manifest_names:
                errors.append(f"skills/manifest.toml: duplicate skill {name!r}")
            manifest_names.append(name)
            for field in required:
                if field not in entry or entry[field] in ("", [], None):
                    errors.append(f"skills/manifest.toml: {name}: missing {field}")
            for field in ("primary_for", "defer_to"):
                values = entry.get(field, [])
                if not isinstance(values, list) or not all(isinstance(value, str) for value in values):
                    errors.append(f"skills/manifest.toml: {name}: {field} must be a string array")
            evidence = entry.get("evidence")
            if isinstance(evidence, str) and not (root / evidence).exists():
                errors.append(f"skills/manifest.toml: {name}: evidence path does not exist: {evidence}")
            for target in entry.get("defer_to", []):
                if target not in names:
                    errors.append(f"skills/manifest.toml: {name}: unknown deferred skill {target!r}")
        if set(manifest_names) != set(names):
            missing = sorted(set(names) - set(manifest_names))
            stale = sorted(set(manifest_names) - set(names))
            if missing:
                errors.append(f"skills/manifest.toml: missing entries: {', '.join(missing)}")
            if stale:
                errors.append(f"skills/manifest.toml: stale entries: {', '.join(stale)}")

    if readme.is_file():
        indexed_links = [
            local_link_path(link)
            for link in markdown_links(readme)
            if local_link_path(link) is not None
        ]
        indexed_targets = set(indexed_links)
        for name in sorted(names):
            if f"{name}/SKILL.md" not in indexed_targets:
                errors.append(f"{name}: missing entry in skills/README.md")

    markdown_files = [readme, skills_dir / "START-HERE.md", *skill_paths]
    for path in markdown_files:
        if not path.is_file():
            errors.append(f"{path.relative_to(root)} is missing")
            continue
        for link in markdown_links(path):
            target = local_link_path(link)
            if target is None:
                continue
            resolved = (path.parent / target).resolve()
            try:
                resolved.relative_to(root.resolve())
            except ValueError:
                errors.append(f"{path.relative_to(root)}: link escapes repository: {link}")
                continue
            if not resolved.exists():
                errors.append(f"{path.relative_to(root)}: broken local link: {link}")

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    root = args.root.resolve()
    errors = validate(root)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        print(f"skill catalogue invalid: {len(errors)} error(s)", file=sys.stderr)
        return 1
    count = len(list((root / "skills").glob("*/SKILL.md")))
    print(f"skill catalogue valid: {count} skills, frontmatter/index/links/manifest checked")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
