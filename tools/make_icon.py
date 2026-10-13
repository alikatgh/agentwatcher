#!/usr/bin/env python3
"""Generate the AgentWatcher app icon (radar/watch motif) as a 1024px PNG."""
import sys
from PIL import Image, ImageDraw

size = 1024
out = sys.argv[1] if len(sys.argv) > 1 else "icon-master.png"

img = Image.new("RGBA", (size, size), (0, 0, 0, 0))

# Rounded dark background with a subtle vertical gradient.
background = Image.new("RGBA", (size, size), (0, 0, 0, 0))
paint = ImageDraw.Draw(background)
for y in range(size):
    t = y / size
    paint.line([(0, y), (size, y)],
               fill=(int(18 + 12 * t), int(20 + 14 * t), int(27 + 18 * t), 255))
mask = Image.new("L", (size, size), 0)
ImageDraw.Draw(mask).rounded_rectangle([30, 30, size - 30, size - 30], radius=210, fill=255)
img.paste(background, (0, 0), mask)

draw = ImageDraw.Draw(img)
center = size // 2

# Concentric rings (teal -> sky -> blue).
rings = [(150, 42, (94, 234, 212, 240)),
         (262, 34, (56, 189, 248, 225)),
         (374, 26, (59, 130, 246, 205))]
for radius, width, color in rings:
    draw.ellipse([center - radius, center - radius, center + radius, center + radius],
                 outline=color, width=width)

# Radar sweep.
draw.line([(center, center), (center + 330, center - 150)], fill=(165, 243, 252, 215), width=20)
draw.line([(center, center), (center + 150, center + 330)], fill=(165, 243, 252, 120), width=20)

# Center dot.
draw.ellipse([center - 62, center - 62, center + 62, center + 62], fill=(255, 255, 255, 255))
draw.ellipse([center - 34, center - 34, center + 34, center + 34], fill=(45, 212, 191, 255))

img.save(out)
print("icon written:", out)
