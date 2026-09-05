#!/usr/bin/env python3
"""生成 lyyIme 的图标 SVG:把文字字形转成矢量 <path>(而非 <text>)。

<text> 在 ibus 切换弹窗等大尺寸场景按字号栅格化再拉伸,会发糊;
字形轮廓是纯矢量,任意尺寸锐利。依赖 fontTools(pip3 install fonttools)。

用法示例:
  python3 scripts/gen-glyph-icon.py --text 伍 --out ibus-engine/icons/lyyime.svg
  python3 scripts/gen-glyph-icon.py --text 中 --out xim/res/zh.svg --size 24
  python3 scripts/gen-glyph-icon.py --text EN --out ibus-engine/icons/lyyime-en.svg
"""
import argparse
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.misc.transform import Transform
from fontTools.ttLib import TTFont, TTCollection


def load_face(font_path: str, want: str):
    """按名字在 ttc/otf/ttf 中挑字体面;want 为 None 取第 0 面。"""
    if font_path.endswith((".ttc", ".otc")):
        coll = TTCollection(font_path)
        for i, f in enumerate(coll.fonts):
            name = f["name"].getDebugName(4) or ""
            if want and want in name:
                return f
        return coll.fonts[0]
    return TTFont(font_path)


def glyph_path(font, gname: str, transform: Transform) -> str:
    """取单字字形轮廓(按字形名),经 transform 后输出 SVG path d 串。"""
    glyph_set = font.getGlyphSet()
    spen = SVGPathPen(glyph_set)
    gpen = TransformPen(spen, transform)
    glyph_set[gname].draw(gpen)
    return spen.getCommands()


def gen(text: str, size: int, bg: str, fg: str, radius: int, font_path: str,
        font_face: str) -> str:
    font = load_face(font_path, font_face)
    upem = font["head"].unitsPerEm
    hmtx = font["hmtx"]
    glyph_set = font.getGlyphSet()
    cmap = font.getBestCmap()

    # 整串排布:先按字体单位量总宽
    names = [cmap[ord(c)] for c in text]
    total_units = sum(hmtx[n][0] for n in names)
    # 字形主体约占 em 的 0.88,留边后缩放到画布
    body = size * (1 - 2 * 0.14)
    s = body / upem
    x0 = (size - total_units * s) / 2
    # CJK 字形位于基线上方约 0.88em、下方 0.12em:整体视觉居中
    baseline = (size - body) / 2 + 0.88 * body

    paths = []
    x = x0
    for n in names:
        t = Transform(s, 0, 0, -s, x, baseline)  # 字体 Y 轴向上,SVG 向下,翻转
        d = glyph_path(font, n, t)
        if d:
            paths.append(f'<path d="{d}" fill="{fg}"/>')
        x += hmtx[n][0] * s

    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        f'<!-- lyyIme 图标:字形已转矢量路径(勿改回 <text>,大尺寸会糊) -->\n'
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}">\n'
        f'  <rect x="0" y="0" width="{size}" height="{size}" rx="{radius}" fill="{bg}"/>\n'
        f'  {"".join(paths)}\n'
        '</svg>\n'
    )


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--text", required=True, help="图标文字,如 伍 / 中 / EN")
    ap.add_argument("--out", required=True)
    ap.add_argument("--size", type=int, default=64, help="viewBox 边长")
    ap.add_argument("--bg", default="#3584e4")
    ap.add_argument("--fg", default="#ffffff")
    ap.add_argument("--radius", type=int, default=14)
    ap.add_argument("--font", default="/usr/share/fonts/google-noto-cjk/NotoSansCJK-Bold.ttc")
    ap.add_argument("--font-face", default="Noto Sans CJK SC Bold",
                    help="ttc 内目标字体面名称(子串匹配)")
    a = ap.parse_args()
    svg = gen(a.text, a.size, a.bg, a.fg, a.radius, a.font, a.font_face)
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(svg)
    print(f"OK {a.out}")


if __name__ == "__main__":
    sys.exit(main())
