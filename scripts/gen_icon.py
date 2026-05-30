from PIL import Image, ImageDraw

SIZE = 1024
CORNER_RADIUS = 220
BG_COLOR = (29, 29, 31, 255)
GOLD = (224, 165, 47, 255)

img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

mask = Image.new("L", (SIZE, SIZE), 0)
mask_draw = ImageDraw.Draw(mask)
mask_draw.rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=CORNER_RADIUS, fill=255)

bg = Image.new("RGBA", (SIZE, SIZE), BG_COLOR)
img.paste(bg, mask=mask)

bars = [
    {"rel_x": 0.22, "rel_h": 0.30},
    {"rel_x": 0.33, "rel_h": 0.55},
    {"rel_x": 0.44, "rel_h": 0.75},
    {"rel_x": 0.55, "rel_h": 0.60},
    {"rel_x": 0.66, "rel_h": 0.38},
    {"rel_x": 0.77, "rel_h": 0.22},
]

bar_w = int(SIZE * 0.065)
bar_radius = bar_w // 2
center_y = SIZE // 2

draw2 = ImageDraw.Draw(img)
for bar in bars:
    cx = int(SIZE * bar["rel_x"])
    bh = int(SIZE * bar["rel_h"])
    x0 = cx - bar_w // 2
    x1 = cx + bar_w // 2
    y0 = center_y - bh // 2
    y1 = center_y + bh // 2
    draw2.rounded_rectangle([x0, y0, x1, y1], radius=bar_radius, fill=GOLD)

img.save("/Users/pedro/Documents/projects/splitter/.claude/worktrees/phase8/src-tauri/app-icon.png", "PNG")
print("Generated 1024x1024 app-icon.png")
