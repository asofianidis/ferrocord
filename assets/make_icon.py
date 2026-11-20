"""Draws the ferrocord app icon: assets/icon.png and assets/icon.ico.

Run with `python assets/make_icon.py` (needs Pillow).
"""
from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024
HERE = Path(__file__).parent


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def draw() -> Image.Image:
    # Vertical Blossom gradient, pink to deep rose.
    top, bottom = (232, 82, 150), (150, 30, 92)
    grad = Image.new("RGBA", (SIZE, SIZE))
    g = ImageDraw.Draw(grad)
    for y in range(SIZE):
        g.line([(0, y), (SIZE, y)], fill=lerp(top, bottom, y / (SIZE - 1)) + (255,))

    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=230, fill=255)
    icon = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    icon.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(icon)
    # Chat bubble with a tail at the bottom left.
    d.rounded_rectangle([190, 220, 834, 700], radius=150, fill="white")
    d.polygon([(300, 640), (250, 840), (470, 690)], fill="white")
    # Lightning bolt: fast.
    bolt = [(560, 290), (400, 490), (505, 490), (450, 640), (630, 420), (520, 420), (585, 290)]
    d.polygon(bolt, fill=bottom + (255,))
    return icon


def main():
    icon = draw()
    icon.save(HERE / "icon.png")
    icon.save(HERE / "icon.ico", sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])


if __name__ == "__main__":
    main()
