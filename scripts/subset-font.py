#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Cut the prose face the window ships out of the vendored one.

The DDLC kit's Mya Nunito carries Latin, Cyrillic, Greek, arrows, mathematical operators and
mathematical alphanumerics in one 459 KB file that the browser downloads whole before the first
paint. The window draws Russian and English, so the letters it always needs go into a small file
of their own, and every other glyph the kit's face carries stays in a second file that the
browser asks for only when such a character reaches the screen. No glyph is dropped: the two
files together cover exactly the code points of the original.

The vendored file stays the source of truth. Run `just subset-font` after the kit is bumped, and
`just check` proves the committed files still match this script.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "site/assets/vendor/ddlc/MyaNunito.woff2"
TARGET = ROOT / "crates/molva-gui/ui/src/assets/fonts"
CORE_FILE = "MyaNunito-window-core.woff2"
REST_FILE = "MyaNunito-window-rest.woff2"

# Latin with its extensions, combining marks, Cyrillic, Vietnamese Latin, punctuation, currency,
# the numero sign, arrows, mathematical operators and the shapes and dingbats the interface uses.
# A dictated phrase, a menu and a table all stay inside these blocks
CORE_BLOCKS = [
    (0x0000, 0x024F),
    (0x0300, 0x036F),
    (0x0400, 0x052F),
    (0x1E00, 0x1EFF),
    (0x2000, 0x206F),
    (0x20A0, 0x20BF),
    (0x2116, 0x2116),
    (0x2122, 0x2122),
    (0x2190, 0x21FF),
    (0x2200, 0x22FF),
    (0x25A0, 0x27BF),
]


def codepoints(font: TTFont) -> set[int]:
    points: set[int] = set()
    for table in font["cmap"].tables:
        points |= set(table.cmap.keys())
    return points


def ranges(points: set[int]) -> list[tuple[int, int]]:
    """Contiguous runs of code points, with a single gap closed to keep the list short"""
    merged: list[tuple[int, int]] = []
    for point in sorted(points):
        if merged and point - merged[-1][1] <= 2:
            merged[-1] = (merged[-1][0], point)
        else:
            merged.append((point, point))
    return merged


def css_range(points: set[int]) -> str:
    parts = []
    for low, high in ranges(points):
        parts.append(f"U+{low:04X}" if low == high else f"U+{low:04X}-{high:04X}")
    return ", ".join(parts)


def unicodes_argument(points: set[int]) -> str:
    return ",".join(
        f"U+{low:04X}" if low == high else f"U+{low:04X}-{high:04X}"
        for low, high in ranges(points)
    )


def subset(points: set[int], destination: Path) -> None:
    subprocess.run(
        [
            sys.executable,
            "-m",
            "fontTools.subset",
            str(SOURCE),
            f"--unicodes={unicodes_argument(points)}",
            "--layout-features=*",
            "--flavor=woff2",
            f"--output-file={destination}",
        ],
        check=True,
    )


def main() -> int:
    check = "--check" in sys.argv[1:]
    original = TTFont(SOURCE)
    everything = codepoints(original)
    core = {
        point
        for point in everything
        if any(low <= point <= high for low, high in CORE_BLOCKS)
    }
    rest = everything - core
    if not core or not rest:
        print("subset-font: the blocks no longer split the face", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix="molva-font-") as workdir:
        built_core = Path(workdir) / CORE_FILE
        built_rest = Path(workdir) / REST_FILE
        subset(core, built_core)
        subset(rest, built_rest)
        if check:
            for name, built in ((CORE_FILE, built_core), (REST_FILE, built_rest)):
                committed = TARGET / name
                if not committed.exists() or committed.read_bytes() != built.read_bytes():
                    print(
                        f"subset-font: {name} does not match the vendored face: run `just subset-font`",
                        file=sys.stderr,
                    )
                    return 1
            print(f"subset-font: {CORE_FILE} and {REST_FILE} match the vendored face")
            return 0

        TARGET.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(built_core, TARGET / CORE_FILE)
        shutil.copyfile(built_rest, TARGET / REST_FILE)

    print(f"core: {len(core)} code points, {TARGET.joinpath(CORE_FILE).stat().st_size} bytes")
    print(f"rest: {len(rest)} code points, {TARGET.joinpath(REST_FILE).stat().st_size} bytes")
    print()
    print("The rest face covers:")
    print(css_range(rest))
    return 0


if __name__ == "__main__":
    sys.exit(main())
