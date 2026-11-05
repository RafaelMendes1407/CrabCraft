#!/usr/bin/env python3
"""Copy the repository's license/asset notices into a package, retaining relative links."""
from pathlib import Path
import shutil
import sys


def main():
    if len(sys.argv) != 2:
        raise SystemExit("Usage: python scripts/copy_notices.py PACKAGE_DIRECTORY")
    root = Path(__file__).resolve().parent.parent
    destination = Path(sys.argv[1]).resolve()
    files = [
        Path("LICENSE-MIT"),
        Path("LICENSE-APACHE"),
        Path("THIRD_PARTY_NOTICES.md"),
        Path("assets/icons/NOTICE.md"),
        Path("assets/fonts/README.md"),
        Path("assets/fonts/NotoSansCJK-LICENSE.txt"),
    ]
    files.extend(path.relative_to(root) for path in sorted((root / "assets/fonts/licenses").glob("*.txt")))
    if len(files) < 10:
        raise SystemExit("The bundled font license inventory is incomplete")
    for relative in files:
        source = root / relative
        if source.is_symlink() or not source.is_file():
            raise SystemExit(f"Missing regular notice file: {relative}")
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    print(f"Copied {len(files)} license and notice files to {destination}")


if __name__ == "__main__":
    main()
