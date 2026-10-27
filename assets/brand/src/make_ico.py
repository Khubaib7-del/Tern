"""Builds assets/brand/tern.ico from tern-mark.png (#71).

The mark fills about 94% of the tile's width (it is wider than tall, so
about 75% of its height), on a rounded black tile (22% corner radius) with
a faint 1 px light rim from 32 px up. Strokes are thickened more the
smaller the icon (the wings and tail are thin), so the mark reads at
taskbar sizes. Each size is drawn from the full-resolution mark at 8x and
reduced once, never scaled from another size.

16 and 20 px are the hand-drawn ones, read back from the existing
tern.ico and kept as they are.

Run from the repository root: python3 assets/brand/src/make_ico.py
Needs pillow.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]
# How much of the tile's width the mark spans.
FILL = 0.94
# Extra stroke weight, in output pixels, by size.
WEIGHT = {16: 0.55, 20: 0.6, 24: 0.35, 32: 0.4, 40: 0.6, 48: 0.5, 64: 0.45, 96: 0.35, 128: 0.3, 256: 0.25}
SS = 8


def mark_mask() -> Image.Image:
    src = Image.open(ROOT / "tern-mark.png").convert("L")
    bbox = src.point(lambda v: 255 if v > 128 else 0).getbbox()
    return src.crop(bbox)


def tile(size: int, mark: Image.Image) -> Image.Image:
    big = size * SS
    # The tile.
    radius = round(big * 0.22)
    out = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(out)
    d.rounded_rectangle((0, 0, big - 1, big - 1), radius=radius, fill=(0, 0, 0, 255))
    if size >= 32:
        rim = Image.new("L", (big, big), 0)
        rd = ImageDraw.Draw(rim)
        rd.rounded_rectangle((0, 0, big - 1, big - 1), radius=radius, outline=58, width=SS)
        out = Image.composite(Image.new("RGBA", (big, big), (255, 255, 255, 255)), out, rim)
    # The mark, scaled to the tile and thickened.
    w = round(big * FILL)
    h = round(mark.height * w / mark.width)
    m = mark.resize((w, h), Image.LANCZOS)
    grow = round(WEIGHT[size] * SS)
    if grow >= 1:
        m = m.filter(ImageFilter.MaxFilter(2 * (grow // 2) + 1))
    layer = Image.new("L", (big, big), 0)
    layer.paste(m, ((big - w) // 2, (big - h) // 2))
    white = Image.new("RGBA", (big, big), (255, 255, 255, 255))
    out = Image.composite(white, out, layer)
    small = out.resize((size, size), Image.LANCZOS)
    if size < 32:
        # Crisper edges at small sizes: push the mark's grey edge pixels
        # toward white or black.
        r, g, b, a = small.split()
        r = r.point(lambda v: min(255, max(0, round((v - 40) * 1.35))))
        small = Image.merge("RGBA", (r, r, r, a))
    return small


# Sizes kept from the hand-drawn icon (the forked tail and the eye cut
# are placed by hand there); every other size is drawn here.
HAND_DRAWN = (16, 20)


def main() -> None:
    mark = mark_mask()
    old = Image.open(ROOT / "tern.ico")
    kept = {s: old.ico.getimage((s, s)).convert("RGBA") for s in HAND_DRAWN if (s, s) in old.ico.sizes()}
    images = [kept.get(s) or tile(s, mark) for s in SIZES]
    images[-1].save(
        ROOT / "tern.ico",
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=images[:-1],
    )
    sheet = Image.new("RGBA", (sum(max(s, 64) + 8 for s in SIZES) + 8, 272), (40, 40, 40, 255))
    x = 8
    for s, im in zip(SIZES, images):
        shown = im.resize((max(s, 64), max(s, 64)), Image.NEAREST) if s < 64 else im
        sheet.paste(shown, (x, 8), shown)
        x += shown.width + 8
    sheet.save(ROOT / "src" / "tern-ico-sizes.png")


if __name__ == "__main__":
    main()
