#!/usr/bin/env python3
"""The gate keeper for the Anubis check page: one drawing, three states.

    scripts/anubis-keeper.py config/anubis/img [--eyes STYLE]
        writes checking/passed/denied .svg + .webp; STYLE is one of EYE_STYLES (default: narrow)

The .webp files are what nginx serves in place of Anubis' mascot (README "Bot protection").
"""
import sys

STATES = {
    # eye colour, aura colour + strength, rim colour, show the ankh
    "checking": dict(eye="#ffc53d", aura="#c99a2e", aura_op=0.38, rim="#c9a24a", ankh=False),
    "passed":   dict(eye="#ffd76a", aura="#2fc4b2", aura_op=0.30, rim="#d8b866", ankh=True),
    "denied":   dict(eye="#ff2b1f", aura="#b3121b", aura_op=0.42, rim="#a8342a", ankh=False),
}

GOLD = "#c9a24a"
GOLD_DK = "#7a5b22"
LAPIS = "#1d2f66"
SIL = "#06070b"      # the figure's silhouette
SIL2 = "#0c0e16"     # headdress cloth


# Eye styles. Each draws the left eye (centre about 228,196); the right one is its mirror image.
EYE_STYLES = ("narrow", "slit", "blank", "orb", "horus", "white")
NARROW = "M 211 199.5 Q 228 189.5 245 193.5 Q 230 202.5 211 199.5 Z"


def mix(color, other, t):
    """color blended towards other by t (0..1), both #rrggbb"""
    c = [int(color[i:i + 2], 16) for i in (1, 3, 5)]
    d = [int(other[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(x + (y - x) * t):02x}" for x, y in zip(c, d))


def eye(style, color):
    almond = "M 212 200 Q 226 186 244 193 Q 230 205 212 200 Z"
    kohl = f'<path d="M 212 200 L 196 208" stroke="{GOLD}" stroke-width="2.2" stroke-opacity="0.8"/>'
    if style == "slit":      # glowing almond, cat-slit pupil
        return [f'<path d="{almond}" fill="{color}" filter="url(#glow)"/>', kohl,
                '<ellipse cx="229" cy="196" rx="1.6" ry="5" fill="#140800"/>']
    if style == "blank":     # glowing, no pupil: nothing behind them
        return [f'<path d="{almond}" fill="{color}" filter="url(#glow2)"/>', kohl]
    if style == "narrow":    # half-closed, weighing you up, under a heavy brow; lit from within,
        # the glow is blurred first and then cut to the eye's outline, so none of it spills out
        return [f'<path d="{NARROW}" fill="{mix(color, "#000000", 0.18)}"/>',
                f'<ellipse cx="229" cy="196" rx="10" ry="3.4" fill="{mix(color, "#ffffff", 0.28)}" '
                f'filter="url(#inner)" clip-path="url(#eyeclip)"/>',
                kohl, '<ellipse cx="229" cy="196.5" rx="1.2" ry="2.6" fill="#140800"/>',
                f'<path d="M 208 191 Q 228 181 248 189" stroke="{SIL}" stroke-width="5" fill="none"/>']
    if style == "orb":       # two points of light in the dark
        return [f'<circle cx="229" cy="196" r="9" fill="{color}" opacity="0.35" filter="url(#glow2)"/>',
                f'<circle cx="229" cy="196" r="4.2" fill="{color}" filter="url(#glow)"/>',
                '<circle cx="229" cy="196" r="1.8" fill="#fffbe8"/>']
    if style == "horus":     # painted like the Eye of Horus: brow, long line, the marks below
        g = f'stroke="{GOLD}" stroke-width="2.4" fill="none" stroke-opacity="0.9"'
        return [f'<path d="{almond}" fill="{color}" filter="url(#glow)"/>',
                '<circle cx="229" cy="196" r="3.4" fill="#140800"/>',
                f'<path d="M 212 200 L 188 206" {g}/>', f'<path d="M 209 186 Q 228 178 248 186" {g}/>',
                f'<path d="M 226 203 L 223 222" {g}/>', f'<path d="M 219 203 Q 214 216 204 218 Q 198 218 200 212" {g}/>']
    if style == "white":     # white-hot core inside a coloured blaze
        return [f'<path d="{almond}" fill="{color}" filter="url(#glow2)" transform="translate(228 196) scale(1.25) translate(-228 -196)"/>',
                f'<path d="{almond}" fill="#fff8e6" filter="url(#glow)"/>', kohl]
    raise SystemExit(f"unknown eye style {style!r}; one of: {', '.join(EYE_STYLES)}")


def mirror(points):
    return [(512 - x, y) for x, y in points]


def poly(points):
    return " ".join(f"{x:g},{y:g}" for x, y in points)


def arc(r, cy=340):
    # lower half circle around (256, cy): the broad collar's bands
    return f"M {256 - r} {cy} A {r} {r} 0 0 0 {256 + r} {cy}"


def svg(state, eyes="narrow"):
    s = STATES[state]
    o = []
    a = o.append
    a('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">')
    a(f'<title>Gate keeper ({state})</title>')
    a('<defs>')
    a('<radialGradient id="void" cx="256" cy="300" r="300" gradientUnits="userSpaceOnUse">'
      '<stop offset="0" stop-color="#161a2b"/><stop offset="0.5" stop-color="#090a12"/>'
      '<stop offset="1" stop-color="#020203"/></radialGradient>')
    a(f'<radialGradient id="aura" cx="256" cy="185" r="150" gradientUnits="userSpaceOnUse">'
      f'<stop offset="0" stop-color="{s["aura"]}" stop-opacity="{s["aura_op"]}"/>'
      f'<stop offset="0.55" stop-color="{s["aura"]}" stop-opacity="{s["aura_op"] * 0.35:.3f}"/>'
      f'<stop offset="1" stop-color="{s["aura"]}" stop-opacity="0"/></radialGradient>')
    a('<linearGradient id="stone" x1="0" y1="0" x2="0" y2="1">'
      '<stop offset="0" stop-color="#2b241a"/><stop offset="1" stop-color="#120f0a"/></linearGradient>')
    a('<filter id="glow" x="-100%" y="-100%" width="300%" height="300%">'
      '<feGaussianBlur stdDeviation="5" result="b"/>'
      '<feMerge><feMergeNode in="b"/><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>')
    a('<filter id="inner" x="-50%" y="-200%" width="200%" height="500%"><feGaussianBlur stdDeviation="2.2"/></filter>')
    a(f'<clipPath id="eyeclip"><path d="{NARROW}"/></clipPath>')
    a('<filter id="glow2" x="-150%" y="-150%" width="400%" height="400%">'
      '<feGaussianBlur stdDeviation="8" result="b"/>'
      '<feMerge><feMergeNode in="b"/><feMergeNode in="b"/><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>')
    a('<filter id="soft" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="14"/></filter>')
    # the doorway's opening: everything of the figure stays inside it
    door = [(114, 100), (398, 100), (392, 512), (120, 512)]
    a(f'<clipPath id="door"><polygon points="{poly(door)}"/></clipPath>')
    body = "M 122 512 C 132 432 170 392 210 378 L 302 378 C 342 392 380 432 390 512 Z"
    a(f'<clipPath id="body"><path d="{body}"/></clipPath>')
    nemes = ("M 198 170 C 172 210 158 250 152 300 L 148 392 L 364 392 L 360 300 "
             "C 354 250 340 210 314 170 Z")
    a(f'<clipPath id="nemes"><path d="{nemes}"/></clipPath>')
    a('</defs>')

    # ── the doorway: darkness, aura, dust
    a(f'<polygon points="{poly(door)}" fill="url(#void)"/>')
    a('<g clip-path="url(#door)">')
    a('<rect x="0" y="0" width="512" height="512" fill="url(#aura)"/>')
    dust = [(150, 150, 1.2), (372, 132, 1.0), (140, 260, 0.9), (382, 236, 1.3), (162, 330, 1.0),
            (366, 318, 0.8), (132, 196, 0.8), (388, 176, 0.9), (176, 120, 0.7), (338, 122, 0.8)]
    for x, y, r in dust:
        a(f'<circle cx="{x}" cy="{y}" r="{r}" fill="{s["rim"]}" opacity="0.55"/>')

    a('<g transform="translate(0 58) translate(256 200) scale(0.82) translate(-256 -200)">')
    # ── headdress (nemes), behind the head
    a(f'<path d="{nemes}" fill="{SIL2}" stroke="{s["rim"]}" stroke-opacity="0.35" stroke-width="1.5"/>')
    a(f'<g clip-path="url(#nemes)" stroke="{GOLD}" stroke-opacity="0.28" stroke-width="2.2" fill="none">')
    for y in range(206, 392, 15):
        a(f'<path d="M 140 {y} Q 256 {y + 10} 372 {y}"/>')
    a('</g>')

    # ── body, shoulders, broad collar
    a(f'<path d="{body}" fill="{SIL}" stroke="{s["rim"]}" stroke-opacity="0.4" stroke-width="1.5"/>')
    a('<g clip-path="url(#body)" fill="none">')
    for r, color, w, op, dash in [(60, GOLD, 5, 0.85, None), (72, LAPIS, 8, 1, None),
                                  (84, GOLD, 3, 0.7, None), (96, GOLD, 6, 0.75, "3 6"),
                                  (108, LAPIS, 6, 1, None), (118, GOLD, 2, 0.6, None)]:
        d = f' stroke-dasharray="{dash}"' if dash else ""
        a(f'<path d="{arc(r)}" stroke="{color}" stroke-width="{w}" stroke-opacity="{op}"{d}/>')
    a('</g>')
    # neck
    a(f'<path d="M 228 292 L 284 292 L 290 352 Q 256 362 222 352 Z" fill="{SIL}"/>')
    # lappets of the headdress, falling in front of the shoulders
    lap = [(198, 250), (228, 266), (224, 384), (186, 386)]
    for pts in (lap, mirror(lap)):
        a(f'<polygon points="{poly(pts)}" fill="{SIL2}" stroke="{s["rim"]}" stroke-opacity="0.35" stroke-width="1.2"/>')
    a(f'<g stroke="{GOLD}" stroke-opacity="0.45" stroke-width="2">')
    for y in range(272, 384, 13):
        a(f'<line x1="{190 + (y - 250) * 0.04:.1f}" y1="{y}" x2="{226 - (y - 250) * 0.02:.1f}" y2="{y + 2}"/>')
        a(f'<line x1="{512 - 190 - (y - 250) * 0.04:.1f}" y1="{y}" x2="{512 - 226 + (y - 250) * 0.02:.1f}" y2="{y + 2}"/>')
    a('</g>')

    # ── the head: tall ears, long muzzle, all in shadow
    ear = [(238, 160), (198, 40), (182, 198)]
    inner = [(229, 160), (200, 70), (191, 186)]
    for e, i in ((ear, inner), (mirror(ear), mirror(inner))):
        a(f'<polygon points="{poly(e)}" fill="{SIL}" stroke="{s["rim"]}" stroke-opacity="0.6" stroke-width="1.6" stroke-linejoin="miter"/>')
        a(f'<polygon points="{poly(i)}" fill="#0f1119" stroke="{GOLD_DK}" stroke-opacity="0.7" stroke-width="1.2"/>')
    head = ("M 186 178 Q 256 138 326 178 L 334 212 C 326 236 302 248 292 264 "
            "L 276 312 Q 256 326 236 312 L 220 264 C 210 248 186 236 178 212 Z")
    a(f'<path d="{head}" fill="{SIL}" stroke="{s["rim"]}" stroke-opacity="0.65" stroke-width="1.8"/>')
    # muzzle ridge and brow, barely lit
    a(f'<path d="M 256 210 L 256 300" stroke="{s["rim"]}" stroke-opacity="0.22" stroke-width="2"/>')
    a(f'<path d="M 214 182 Q 234 176 248 184 M 298 182 Q 278 176 264 184" fill="none" '
      f'stroke="{s["rim"]}" stroke-opacity="0.35" stroke-width="1.6"/>')
    a('<ellipse cx="256" cy="314" rx="13" ry="7" fill="#000"/>')
    a(f'<ellipse cx="252" cy="311" rx="3.5" ry="1.6" fill="{s["rim"]}" opacity="0.5"/>')
    # cheekbones
    a(f'<path d="M 182 212 L 214 226 M 330 212 L 298 226" stroke="{s["rim"]}" stroke-opacity="0.3" stroke-width="1.5"/>')

    # ── the eyes
    left = "".join(eye(eyes, s["eye"]))
    a(left)
    a(f'<g transform="matrix(-1 0 0 1 512 0)">{left}</g>')

    # ── the was-sceptre at his side
    a(f'<path d="M 356 512 L 356 156" stroke="{GOLD_DK}" stroke-width="7"/>')
    a(f'<path d="M 354.5 512 L 354.5 160" stroke="{GOLD}" stroke-width="2" stroke-opacity="0.8"/>')
    a(f'<path d="M 353 158 L 372 142 L 382 150 L 376 154 L 370 150 L 357 163 Z" fill="{GOLD}" '
      f'stroke="{GOLD_DK}" stroke-width="1"/>')

    a('</g>')  # end figure
    # ── fog rolling in at his feet
    a('<ellipse cx="256" cy="508" rx="190" ry="40" fill="#8a93b8" opacity="0.16" filter="url(#soft)"/>')
    a('<ellipse cx="190" cy="496" rx="90" ry="22" fill="#8a93b8" opacity="0.12" filter="url(#soft)"/>')
    a('<ellipse cx="330" cy="500" rx="100" ry="24" fill="#8a93b8" opacity="0.12" filter="url(#soft)"/>')

    # ── passed: the ankh, life granted
    if s["ankh"]:
        a(f'<g filter="url(#glow)" stroke="{s["eye"]}" stroke-width="7" fill="none" stroke-linecap="butt">')
        a('<ellipse cx="256" cy="420" rx="12" ry="16"/>')
        a('<path d="M 230 440 L 282 440 M 256 436 L 256 488"/>')
        a('</g>')
    a('</g>')  # end door clip

    # ── the gate: battered jambs, lintel with the winged sun, cavetto cornice
    jamb = [(62, 100), (114, 100), (120, 512), (54, 512)]
    for pts in (jamb, mirror(jamb)):
        a(f'<polygon points="{poly(pts)}" fill="url(#stone)" stroke="{GOLD_DK}" stroke-width="1.5"/>')
    for x0, sgn in ((88, 1), (424, -1)):
        # small glyphs down each jamb: sun, ankh, djed
        g = f'stroke="{GOLD}" stroke-opacity="0.55" stroke-width="2" fill="none"'
        a(f'<circle cx="{x0}" cy="150" r="9" {g}/><circle cx="{x0}" cy="150" r="2" fill="{GOLD}" opacity="0.55"/>')
        a(f'<g {g}><ellipse cx="{x0}" cy="206" rx="5" ry="7"/><path d="M {x0 - 9} 215 L {x0 + 9} 215 M {x0} 213 L {x0} 238"/></g>')
        a(f'<g {g}><path d="M {x0} 270 L {x0} 312 M {x0 - 8} 276 L {x0 + 8} 276 M {x0 - 8} 284 L {x0 + 8} 284 '
          f'M {x0 - 8} 292 L {x0 + 8} 292"/></g>')
        a(f'<line x1="{x0 + sgn * 22}" y1="112" x2="{x0 + sgn * 25:.1f}" y2="512" stroke="{GOLD_DK}" stroke-width="1" opacity="0.6"/>')
    a(f'<polygon points="56,72 456,72 456,102 56,102" fill="url(#stone)" stroke="{GOLD_DK}" stroke-width="1.5"/>')
    a(f'<path d="M 56 72 C 50 56 40 46 32 40 L 480 40 C 472 46 462 56 456 72 Z" fill="#1d1811" '
      f'stroke="{GOLD_DK}" stroke-width="1.5"/>')
    a(f'<line x1="56" y1="72" x2="456" y2="72" stroke="{GOLD}" stroke-width="2" opacity="0.7"/>')
    # winged sun disk
    a(f'<circle cx="256" cy="87" r="9" fill="{GOLD}"/>')
    for sgn in (-1, 1):
        x1, x2 = 256 + sgn * 12, 256 + sgn * 92
        a(f'<path d="M {x1} 84 Q {256 + sgn * 50} 76 {x2} 82 Q {256 + sgn * 52} 92 {x1} 92 Z" fill="{GOLD}" opacity="0.85"/>')
        for k in (1, 2, 3):
            xx = 256 + sgn * (12 + 20 * k)
            a(f'<line x1="{xx}" y1="80" x2="{xx + sgn * 6}" y2="91" stroke="#2b241a" stroke-width="1.2"/>')
    a('</svg>')
    return "\n".join(o) + "\n"


if __name__ == "__main__":
    import argparse
    import subprocess
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--eyes", default="narrow", choices=EYE_STYLES)
    args = ap.parse_args()
    out = args.out
    for state in STATES:
        path = f"{out}/{state}"
        with open(f"{path}.svg", "w") as f:
            f.write(svg(state, args.eyes))
        # 512 px for sharp screens (the page shows it at up to 256 px); needs rsvg-convert + ImageMagick
        subprocess.run(["rsvg-convert", "-w", "512", "-h", "512", f"{path}.svg", "-o", f"{path}.png"], check=True)
        subprocess.run(["magick", f"{path}.png", "-quality", "88", f"{path}.webp"], check=True)
        subprocess.run(["rm", f"{path}.png"], check=True)
