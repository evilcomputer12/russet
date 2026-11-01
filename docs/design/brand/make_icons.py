# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Russet contributors
"""Writes Russet's icons from one drawing: a round crab apple in white on a russet app tile, a
straight stem, its leaf a crab's open claw turned left. Round, unbitten and left-leafed, so it is
never mistaken for Apple's mark.

    python3 make_icons.py            # the SVGs, next to this file and in extension/media
    python3 make_icons.py --raster   # also the PNGs and the Windows .ico (rsvg-convert, Pillow)

Variants: `russet-icon-light.svg` (the app tile), `russet-icon-dark.svg` (the tile a shade brighter,
for dark pages), `russet-icon-small.svg` (a larger apple on the tile, for 16-48 px) and
`russet-mono.svg` (the apple alone in `currentColor`, for the IDE's activity bar).
"""
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
MEDIA = HERE.parents[2] / "extension" / "media"
HEADER = "<!-- SPDX-License-Identifier: AGPL-3.0-only -->"

# 256-unit canvas. The tile is a rounded square with the corner of an iOS app icon.
TILE = '<rect x="12" y="12" width="232" height="232" rx="54"/>'
# A round fruit with a shallow dimple at the stem.
APPLE = ("M128 100 C 138 92, 152 90, 163 94 C 186 102, 198 124, 198 150 C 198 186, 167 212, 128 212 "
         "C 89 212, 58 186, 58 150 C 58 124, 70 102, 93 94 C 104 90, 118 92, 128 100 Z")
# The stem, upright, and the leaf as an open pincer to its left (two fingers around a V).
STEM = "M124 102 L 124 62 C 124 58, 132 58, 132 62 L 132 102 Z"
CLAW = ("M124 74 C 108 56, 84 52, 66 60 L 92 70 L 70 82 C 86 92, 108 90, 124 80 Z")

PALETTES = {
    "light": dict(top="#F08A4B", bottom="#B4471D"),
    "dark": dict(top="#FF9A5C", bottom="#C9551F"),
}


def tile(name, scale=1.0):
    p = PALETTES[name]
    glyph = f'<path d="{APPLE}"/><path d="{STEM}"/><path d="{CLAW}"/>'
    if scale != 1.0:
        glyph = f'<g transform="translate(128 140) scale({scale}) translate(-128 -140)">{glyph}</g>'
    return f"""{HEADER}
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{p['top']}"/>
      <stop offset="1" stop-color="{p['bottom']}"/>
    </linearGradient>
  </defs>
  <g fill="url(#tile)">{TILE}</g>
  <g fill="#FFFFFF">{glyph}</g>
</svg>
"""


def mono():
    # The apple alone, cropped to itself, for 16-24 px in the activity bar.
    return f"""{HEADER}
<svg xmlns="http://www.w3.org/2000/svg" viewBox="50 42 156 176" width="24" height="24">
  <g fill="currentColor"><path d="{APPLE}"/><path d="{STEM}"/><path d="{CLAW}"/></g>
</svg>
"""


def letterpress(fill, opacity):
    # The editor's empty-area watermark: the apple alone, faint.
    return f"""{HEADER}
<svg xmlns="http://www.w3.org/2000/svg" viewBox="50 42 156 176">
  <g fill="{fill}" fill-opacity="{opacity}"><path d="{APPLE}"/><path d="{STEM}"/><path d="{CLAW}"/></g>
</svg>
"""


def write(path, text):
    path.write_text(text, encoding="utf-8", newline="\n")


def main():
    out = {
        "russet-icon-light.svg": tile("light"),
        "russet-icon-dark.svg": tile("dark"),
        "russet-icon-small.svg": tile("light", scale=1.18),
        "russet-mono.svg": mono(),
    }
    for name, text in out.items():
        write(HERE / name, text)
    write(MEDIA / "russet.svg", out["russet-mono.svg"])
    # The editor's own branding (russet-windows/scripts/brand-editor.ps1 installs it).
    editor = HERE / "editor"
    editor.mkdir(exist_ok=True)
    for theme, fill, opacity in (("dark", "#FFFFFF", 0.07), ("light", "#000000", 0.07),
                                 ("hcDark", "#FFFFFF", 0.3), ("hcLight", "#000000", 0.3)):
        write(editor / f"letterpress-{theme}.svg", letterpress(fill, opacity))
    if "--raster" not in sys.argv:
        return
    from PIL import Image

    def png(svg, size, dest):
        subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(dest), str(HERE / svg)], check=True)
        return Image.open(dest)

    png("russet-icon-light.svg", 128, MEDIA / "russet-icon.png")
    png("russet-icon-light.svg", 512, HERE / "russet-icon-512.png")
    # Start menu tiles (VisualElementsManifest).
    png("russet-icon-light.svg", 150, editor / "code_150x150.png")
    png("russet-icon-small.svg", 70, editor / "code_70x70.png")
    # Windows icon: the larger apple up to 48 px, the tile as drawn above.
    frames = [png("russet-icon-small.svg" if s <= 48 else "russet-icon-light.svg", s, HERE / f".ico-{s}.png")
              for s in (16, 24, 32, 48, 64, 128, 256)]
    frames[-1].save(HERE / "russet.ico", sizes=[f.size for f in frames], append_images=frames[:-1])
    for f in HERE.glob(".ico-*.png"):
        f.unlink()


if __name__ == "__main__":
    main()
