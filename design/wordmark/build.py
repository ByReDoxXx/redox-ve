"""Yazı logoyu (RedoX VE) yeşil perdeli kaynaktan üretir.

    python design/wordmark/build.py

Kaynak: design/wordmark/kaynak.png (yeşil zeminli). Çıktı: crates/redox-server/assets/wordmark.svg
(şeffaf PNG'yi içine gömülü taşıyan SVG; uygulama tek dosya olarak sunar) ve design/wordmark/wordmark.png.
"""
import base64
import io
import os

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "..", "crates", "redox-server", "assets", "wordmark.svg")
HEIGHT = 200  # giriş sayfasındaki 58 pikselin üç katından büyük: yüksek çözünürlüklü ekranda da net

src = Image.open(os.path.join(HERE, "kaynak.png")).convert("RGB")
w, h = src.size
out = Image.new("RGBA", (w, h))
LO, HI = 24, 96  # yeşilin kırmızı ve maviden fazlası: LO altı tam opak, HI üstü tam saydam
px, po = src.load(), out.load()
for y in range(h):
    for x in range(w):
        r, g, b = px[x, y]
        excess = g - max(r, b)
        if excess <= LO:
            po[x, y] = (r, g, b, 255)
        elif excess >= HI:
            po[x, y] = (0, 0, 0, 0)
        else:
            alpha = int(255 * (HI - excess) / (HI - LO))
            # Kenardaki yeşil yansımayı bastır: yeşil, diğer iki kanalın büyüğünü geçmesin.
            po[x, y] = (r, min(g, max(r, b)), b, alpha)

box = out.split()[3].point(lambda a: 255 if a > 16 else 0).getbbox()
out = out.crop(box)
width = round(out.width * HEIGHT / out.height)
out = out.resize((width, HEIGHT), Image.LANCZOS)
out.save(os.path.join(HERE, "wordmark.png"), optimize=True)
buf = io.BytesIO()
out.save(buf, "PNG", optimize=True)
data = base64.b64encode(buf.getvalue()).decode("ascii")
svg = (
    f'<svg version="1.1" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {HEIGHT}" role="img" aria-label="RedoX VE">'
    f'<image width="{width}" height="{HEIGHT}" href="data:image/png;base64,{data}"/></svg>\n'
)
io.open(OUT, "w", encoding="utf-8", newline="\n").write(svg)
print(f"wordmark.svg: {width}x{HEIGHT}, {len(svg) // 1024} KiB; oran {width / HEIGHT:.3f}")
