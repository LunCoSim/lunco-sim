"""Verify manifest-staged assets survive Windows installer/update packaging."""

import argparse
import hashlib
from pathlib import Path
import subprocess
import tempfile
import zipfile


def expected_files(reference: Path) -> dict[str, str]:
    files = {}
    for path in sorted(reference.rglob("*")):
        if path.is_file():
            with path.open("rb") as stream:
                files[path.relative_to(reference).as_posix()] = hashlib.file_digest(
                    stream, "sha256"
                ).hexdigest()
    if not files:
        raise ValueError(f"No manifest bundle files staged in {reference}")
    return files


def verify_update(package: Path, expected: dict[str, str]) -> None:
    with zipfile.ZipFile(package) as archive:
        for relative, digest in expected.items():
            name = f"lib/app/assets/{relative}"
            if archive.namelist().count(name) != 1:
                raise ValueError(f"{package.name}: expected exactly one {name}")
            with archive.open(name) as stream:
                if hashlib.file_digest(stream, "sha256").hexdigest() != digest:
                    raise ValueError(f"{package.name}: changed bundle file {name}")


def verify_installed_assets(assets: Path, expected: dict[str, str]) -> None:
    for relative, digest in expected.items():
        path = assets / relative
        if not path.is_file():
            raise ValueError(f"MSI: missing bundle file {relative}")
        with path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != digest:
                raise ValueError(f"MSI: changed bundle file {relative}")


def verify_msi(package: Path, expected: dict[str, str]) -> None:
    # Administrative extraction exercises the real MSI cabinet without
    # installing or launching the application on the build machine.
    with tempfile.TemporaryDirectory(prefix="lunco-msi-assets-") as directory:
        root = Path(directory)
        target = root / "payload"
        target.mkdir()
        log = root / "extract.log"
        result = subprocess.run([
            "msiexec.exe", "/a", str(package.resolve()), "/qn", "/norestart",
            f"TARGETDIR={target}", "/L*v", str(log),
        ])
        if result.returncode:
            if log.is_file():
                print(log.read_text(encoding="utf-16", errors="replace")[-8000:])
            raise ValueError(f"MSI administrative extraction failed: {result.returncode}")
        roots = [path / "assets" for path in target.rglob("current")
                 if (path / "assets").is_dir()]
        if len(roots) != 1:
            raise ValueError(f"{package.name}: expected exactly one current/assets tree")
        verify_installed_assets(roots[0], expected)


def single_package(directory: Path, pattern: str) -> Path:
    matches = list(directory.glob(pattern))
    if len(matches) != 1:
        raise ValueError(f"Expected exactly one {pattern} in {directory}, found {len(matches)}")
    return matches[0]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("packages", type=Path)
    parser.add_argument("--msi", action="store_true")
    args = parser.parse_args()
    expected = expected_files(args.reference)
    verify_update(single_package(args.packages, "*-full.nupkg"), expected)
    if args.msi:
        verify_msi(single_package(args.packages, "*.msi"), expected)
    print(f"Verified {len(expected)} manifest bundle files in Windows package payloads")


if __name__ == "__main__":
    main()
