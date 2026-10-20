"""Draws the JP app icon. Run: python assets/make_icon.py (needs Pillow, Windows fonts)."""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).parent
SIZE = 512
BG = (42, 120, 214)  # theme accent #2a78d6
FONT = "C:/Windows/Fonts/segoeuib.ttf"

img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=SIZE // 5, fill=BG)
font = ImageFont.truetype(FONT, int(SIZE * 0.56))
box = d.textbbox((0, 0), "JP", font=font)
w, h = box[2] - box[0], box[3] - box[1]
d.text(((SIZE - w) / 2 - box[0], (SIZE - h) / 2 - box[1]), "JP", font=font, fill="white")

img.resize((256, 256), Image.LANCZOS).save(HERE / "icon.png")
img.save(HERE / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
