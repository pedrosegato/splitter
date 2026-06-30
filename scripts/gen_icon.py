from PIL import Image, ImageDraw

SIZE = 1024
MARGIN = 100
INNER = SIZE - MARGIN * 2
CORNER_RADIUS = int(INNER * 0.2237)
BG_COLOR = (29, 29, 31, 255)
GOLD = (224, 165, 47, 255)
OUTPUT = "/Users/pedro/Documents/projects/splitter/src-tauri/app-icon.png"

img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))

mask = Image.new("L", (SIZE, SIZE), 0)
ImageDraw.Draw(mask).rounded_rectangle(
    [MARGIN, MARGIN, SIZE - MARGIN, SIZE - MARGIN],
    radius=CORNER_RADIUS,
    fill=255,
)
img.paste(Image.new("RGBA", (SIZE, SIZE), BG_COLOR), mask=mask)

bars = [
    {"rel_x": 0.22, "rel_h": 0.30},
    {"rel_x": 0.33, "rel_h": 0.55},
    {"rel_x": 0.44, "rel_h": 0.75},
    {"rel_x": 0.55, "rel_h": 0.60},
    {"rel_x": 0.66, "rel_h": 0.38},
    {"rel_x": 0.77, "rel_h": 0.22},
]

bar_w = int(INNER * 0.065)
bar_radius = bar_w // 2
center_y = SIZE // 2

draw = ImageDraw.Draw(img)
for bar in bars:
    cx = MARGIN + int(INNER * bar["rel_x"])
    bh = int(INNER * bar["rel_h"])
    draw.rounded_rectangle(
        [cx - bar_w // 2, center_y - bh // 2, cx + bar_w // 2, center_y + bh // 2],
        radius=bar_radius,
        fill=GOLD,
    )

img.save(OUTPUT, "PNG")
print(f"Generated {SIZE}x{SIZE} -> {OUTPUT} (squircle {INNER}px, margin {MARGIN}px)")
