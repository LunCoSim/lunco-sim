#!/usr/bin/env python3
"""Split Velopack build output into human downloads and updater assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class RuntimeRelease:
    channel: str
    installer_suffix: str
    public_name: str


RUNTIMES = (
    RuntimeRelease("linux-x64", ".AppImage", "LunCoSim-Linux-x86_64.AppImage"),
    RuntimeRelease("osx-arm64", "-Setup.pkg", "LunCoSim-macOS-Apple-Silicon.pkg"),
    RuntimeRelease("osx-x64", "-Setup.pkg", "LunCoSim-macOS-Intel.pkg"),
    RuntimeRelease("win-x64", "-Setup.exe", "LunCoSim-Windows-x86_64-Setup.exe"),
)


class StagingError(RuntimeError):
    pass


def _empty_output(path: Path) -> None:
    if path.exists() and any(path.iterdir()):
        raise StagingError(f"output directory is not empty: {path}")
    path.mkdir(parents=True, exist_ok=True)


def _one(paths: list[Path], description: str) -> Path:
    if len(paths) != 1:
        found = ", ".join(str(path) for path in paths) or "none"
        raise StagingError(f"expected exactly one {description}; found {found}")
    return paths[0]


def _load_feed(path: Path) -> list[dict[str, object]]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise StagingError(f"cannot read Velopack feed {path}: {error}") from error
    assets = document.get("Assets") if isinstance(document, dict) else None
    if not isinstance(assets, list) or not assets:
        raise StagingError(f"Velopack feed has no Assets: {path}")
    if not all(isinstance(asset, dict) for asset in assets):
        raise StagingError(f"Velopack feed contains a non-object asset: {path}")
    return assets


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _verify_asset(asset: dict[str, object], package: Path, feed: Path) -> None:
    expected_size = asset.get("Size")
    if isinstance(expected_size, int) and package.stat().st_size != expected_size:
        raise StagingError(
            f"size mismatch for {package.name} referenced by {feed.name}: "
            f"expected {expected_size}, got {package.stat().st_size}"
        )

    expected_sha256 = asset.get("SHA256")
    if isinstance(expected_sha256, str) and expected_sha256:
        actual = _sha256(package)
        if actual.casefold() != expected_sha256.casefold():
            raise StagingError(
                f"SHA256 mismatch for {package.name} referenced by {feed.name}"
            )


def stage_release(input_root: Path, public_dir: Path, updates_dir: Path) -> None:
    if not input_root.is_dir():
        raise StagingError(f"artifact input directory does not exist: {input_root}")
    _empty_output(public_dir)
    _empty_output(updates_dir)

    copied_update_names: set[str] = set()
    for runtime in RUNTIMES:
        feed = _one(
            list(input_root.rglob(f"releases.{runtime.channel}.json")),
            f"releases.{runtime.channel}.json feed",
        )
        assets = _load_feed(feed)

        installer = _one(
            [
                path
                for path in feed.parent.iterdir()
                if path.is_file() and path.name.endswith(runtime.installer_suffix)
            ],
            f"{runtime.channel} installer ending in {runtime.installer_suffix}",
        )
        shutil.copy2(installer, public_dir / runtime.public_name)

        shutil.copy2(feed, updates_dir / feed.name)
        copied_update_names.add(feed.name)
        full_packages = 0
        for asset in assets:
            file_name = asset.get("FileName")
            if not isinstance(file_name, str) or not file_name:
                raise StagingError(f"asset in {feed} has no FileName")
            if Path(file_name).name != file_name:
                raise StagingError(f"unsafe asset FileName in {feed}: {file_name}")
            package = feed.parent / file_name
            if not package.is_file():
                raise StagingError(f"{feed.name} references missing package {file_name}")
            _verify_asset(asset, package, feed)
            destination = updates_dir / file_name
            if file_name in copied_update_names:
                if (
                    destination.stat().st_size != package.stat().st_size
                    or _sha256(destination) != _sha256(package)
                ):
                    raise StagingError(f"conflicting updater package name: {file_name}")
            else:
                shutil.copy2(package, destination)
                copied_update_names.add(file_name)
            if asset.get("Type") == "Full":
                full_packages += 1
        if full_packages == 0:
            raise StagingError(f"Velopack feed has no full package: {feed}")

    public_names = {path.name for path in public_dir.iterdir() if path.is_file()}
    expected_public_names = {runtime.public_name for runtime in RUNTIMES}
    if public_names != expected_public_names:
        raise StagingError(
            f"public release set differs from the four supported installers: {public_names}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--public", type=Path, required=True)
    parser.add_argument("--updates", type=Path, required=True)
    args = parser.parse_args()
    try:
        stage_release(args.input, args.public, args.updates)
    except StagingError as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
