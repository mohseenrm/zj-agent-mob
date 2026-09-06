#!/usr/bin/env python3
"""Rasterise captured terminal frames into a GIF.

    python3 scripts/demo/render-frames.py <framedir> <out.gif>

`<framedir>` holds the frames captured by capture-tour.sh: one `NNNN.txt` per
frame, each an ANSI dump of the panel taken with `zellij action dump-screen
--ansi`, plus a `NNNN.hold` naming how many frame-times that frame stays up.

No terminal is recorded and no browser is involved: the tour is driven with
`zellij action`, the panel's own output is read back as text, and this turns
that text into pixels. That means the frames are exactly what the plugin
rendered, the render is deterministic, and a frame can be re-inspected as text
long after the fact.
"""
import os
import re
import sys

from PIL import Image, ImageDraw, ImageFont

FONT_DIR = os.path.expanduser("~/Library/Fonts")
FONT_REG = os.path.join(FONT_DIR, "JetBrainsMonoNerdFontMono-Regular.ttf")
FONT_BOLD = os.path.join(FONT_DIR, "JetBrainsMonoNerdFontMono-Bold.ttf")

# The panel's glyph vocabulary that JetBrains Mono renders as a box.
#
# It reports coverage for all of them - `getmask(ch).getbbox()` is non-empty -
# but what it draws is the .notdef rectangle: rendered in isolation `⠇`, `↵` and
# `⏵` produce byte-identical ink, while a font with real glyphs gives three
# different values. So coverage is established by reading each font's cmap with
# fontTools, not by asking PIL to render.
#
# Menlo covers `↵` only. Not a general fallback: `❯` is in JetBrains Mono and
# looks wrong in Menlo, and the media glyphs below are in neither.
FONT_FALLBACK = "/System/Library/Fonts/Menlo.ttc"
FALLBACK_CHARS = set("↵")

# Drawn, not typeset: no font on this machine has these in its cmap, so they
# came out as .notdef boxes. A disc, a triangle and two bars.
MEDIA_CHARS = set("⏺⏵⏸")


def draw_media(d, ch, x, y, cw, lh, fill):
    """Draw the transcript's status glyphs as shapes."""
    cx, cy = x + cw / 2, y + lh / 2
    r = cw * 0.30
    if ch == "⏺":
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fill)
    elif ch == "⏵":
        d.polygon([(cx - r, cy - r), (cx - r, cy + r), (cx + r, cy)], fill=fill)
    else:
        w = r * 0.42
        d.rectangle([cx - r, cy - r, cx - r + w, cy + r], fill=fill)
        d.rectangle([cx + r - w, cy - r, cx + r, cy + r], fill=fill)

# Braille is drawn too. Apple Symbols is the only font here with the block and
# renders it at half a cell, top-aligned, so the spinner came out as a speck.
# Bit order is Unicode's: bits 0-2 left column, 3-5 right, 6/7 the fourth row.
BRAILLE_CHARS = {chr(c) for c in range(0x2800, 0x2900)}
BRAILLE_DOTS = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)]


def draw_braille(d, ch, x, y, cw, lh, fill):
    """Draw one Braille cell as its 2x4 dot matrix."""
    bits = ord(ch) - 0x2800
    r = max(1.0, cw / 7.0)
    # Inset so the matrix sits inside the cell with the same optical weight as a
    # glyph, and centred on the cell rather than hung from its top.
    span_x, span_y = cw * 0.46, lh * 0.52
    ox = x + (cw - span_x) / 2
    oy = y + (lh - span_y) / 2
    for i, (col, row) in enumerate(BRAILLE_DOTS):
        if not bits & (1 << i):
            continue
        cx = ox + col * span_x
        cy = oy + row * (span_y / 3)
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=fill)

FONT_SIZE = 20
PAD = 28
# Tokyo Night, to match the Zellij theme the panel is recorded against.
BG = (26, 27, 38)
FG = (169, 177, 214)
WINDOW_BAR = 34
DOT_COLORS = [(255, 95, 86), (255, 189, 46), (39, 201, 63)]

# xterm 256 -> rgb, for the 16 base colours the theme actually uses.
BASE = {
    0: (26, 27, 38), 1: (247, 118, 142), 2: (158, 206, 106), 3: (224, 175, 104),
    4: (122, 162, 247), 5: (187, 154, 247), 6: (125, 207, 255), 7: (169, 177, 214),
    8: (86, 95, 137), 9: (247, 118, 142), 10: (158, 206, 106), 11: (224, 175, 104),
    12: (122, 162, 247), 13: (187, 154, 247), 14: (125, 207, 255), 15: (192, 202, 245),
}


def xterm_rgb(n):
    if n in BASE:
        return BASE[n]
    if 16 <= n <= 231:
        n -= 16
        r, g, b = n // 36, (n // 6) % 6, n % 6
        f = lambda v: 0 if v == 0 else 55 + 40 * v
        return (f(r), f(g), f(b))
    v = 8 + 10 * (n - 232)
    return (v, v, v)


# Colour is applied here rather than captured, because `dump-screen --ansi`
# returns ZERO BYTES for a plugin pane on this Zellij (verified: the same call
# against a terminal pane in the same session returns styled output). The panel
# is a plugin pane, so its colours cannot be read back at all and the dump is
# plain text.
#
# These rules mirror the panel's own palette rather than inventing one: statuses
# carry the colour the panel gives them, detail lines are dim, the selected row
# is highlighted, and the footer ribbon is accented.
STATUS_COLORS = {
    "failed": (247, 118, 142),
    "waiting": (224, 175, 104),
    "idle-wait": (224, 175, 104),
    "working": (224, 175, 104),
    "compact": (187, 154, 247),
    "done": (158, 206, 106),
    "idle": (125, 207, 255),
    "gone": (86, 95, 137),
    "found": (86, 95, 137),
}
DIM = (86, 95, 137)
ACCENT = (158, 206, 106)
HEADER_NAME = (125, 207, 255)
SEL_BG = (41, 46, 66)

ANSI = re.compile(r"\x1b\[([0-9;]*)m")
# Everything else that can appear in a dump and must not become glyphs:
# other CSI sequences, OSC-8 hyperlinks, and DCS/APC blocks.
STRIP = re.compile(
    r"\x1b\][0-9]*;[^\x07\x1b]*(?:\x07|\x1b\\)"
    r"|\x1b[P_][^\x1b]*\x1b\\"
    r"|\x1b\[[0-9;?]*[A-Za-z]"
    r"|\x1b[()][B0]"
    r"|[\x00-\x08\x0b-\x1f\x7f]"
)


def parse(line):
    """ANSI line -> [(text, fg, bold)], keeping colour and weight."""
    out, fg, bold, pos = [], FG, False, 0
    for m in ANSI.finditer(line):
        chunk = line[pos:m.start()]
        if chunk:
            out.append((chunk, fg, bold))
        codes = [int(c) for c in m.group(1).split(";") if c != ""] or [0]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                fg, bold = FG, False
            elif c == 1:
                bold = True
            elif c == 22:
                bold = False
            elif c == 2:
                # Dim: the panel uses it for detail lines. Approximated by
                # blending toward the background, since PIL has no dim.
                fg = tuple((v + b) // 2 for v, b in zip(fg, BG))
            elif 30 <= c <= 37:
                fg = BASE[c - 30]
            elif 90 <= c <= 97:
                fg = BASE[c - 90 + 8]
            elif c == 39:
                fg = FG
            elif c == 38 and i + 1 < len(codes):
                if codes[i + 1] == 5 and i + 2 < len(codes):
                    fg = xterm_rgb(codes[i + 2]); i += 2
                elif codes[i + 1] == 2 and i + 4 < len(codes):
                    fg = tuple(codes[i + 2:i + 5]); i += 4
            i += 1
        pos = m.end()
    tail = line[pos:]
    if tail:
        out.append((tail, fg, bold))
    return out


ROW = re.compile(r"^(\s*[!▶ ]*\s*\d+\s+\S?\s*)(claude|codex|agent)(\s+)(\S+)(\s+.*)$")


def spans(ln):
    """Plain panel line -> [(text, colour, bold)], coloured by what it is."""
    stripped = ln.strip()
    if not stripped:
        return []
    # The jump frame is the agent's own pane, not the panel. Same approach:
    # re-colour its transcript by what each line is.
    if stripped.startswith("✻ Claude Code") or stripped.startswith("codex"):
        head, _, ver = stripped.partition(" v")
        pre = ln[: len(ln) - len(ln.lstrip())]
        out = [(pre, FG, False), (head, HEADER_NAME, True)]
        if ver:
            out.append((" v" + ver, DIM, False))
        return out
    # A tool call the agent ran, and the prompt it is now blocked on.
    if stripped.startswith("⏺"):
        i = ln.index("⏺")
        colour = STATUS_COLORS["waiting"] if "Bash" in ln else ACCENT
        return [(ln[:i], FG, False), ("⏺", colour, True), (ln[i + 1:], FG, False)]
    if stripped.startswith("Do you want to proceed?"):
        return [(ln, STATUS_COLORS["waiting"], True)]
    # The selected choice in that prompt, and the ones not taken.
    if stripped.startswith("❯"):
        return [(ln, ACCENT, True)]
    if re.match(r"^\s*[23]\.\s", ln) or stripped.startswith(">"):
        return [(ln, DIM, False)]
    # Box drawing: separators, the permission-prompt frame, detail elbows.
    if set(stripped) <= set("─│┌┐└┘ "):
        return [(ln, DIM, False)]
    # Header: "zj-agent-mob  1 failed · 2 working"
    if stripped.startswith("zj-agent-mob"):
        out, i = [], ln.index("zj-agent-mob")
        out.append((ln[:i], FG, False))
        out.append(("zj-agent-mob", HEADER_NAME, True))
        rest = ln[i + len("zj-agent-mob"):]
        # Colour each "<n> <status>" pair by that status.
        pos = 0
        for m in re.finditer(r"(\d+)\s+([a-z-]+)", rest):
            if m.group(2) not in STATUS_COLORS:
                continue
            out.append((rest[pos:m.start()], FG, False))
            out.append((m.group(0), STATUS_COLORS[m.group(2)], True))
            pos = m.end()
        out.append((rest[pos:], FG, False))
        return out
    # Detail line, always under a row and always secondary.
    if stripped.startswith("└"):
        return [(ln, DIM, False)]
    # Footer ribbon: "↵ jump  g goto  / find" - key then label.
    if stripped.startswith("↵") or " esc " in ln or stripped.startswith("/"):
        out, pos = [], 0
        for m in re.finditer(r"(\S+)(\s+)(\S+)", ln):
            out.append((ln[pos:m.start()], FG, False))
            out.append((m.group(1), ACCENT, True))
            out.append((m.group(2) + m.group(3), FG, False))
            pos = m.end()
        out.append((ln[pos:], FG, False))
        return out
    # An agent row: colour the tool and the status, leave the rest plain.
    m = ROW.match(ln)
    if m:
        pre, tool, gap, status, rest = m.groups()
        return [
            (pre, ACCENT if "▶" in pre else FG, False),
            (tool, HEADER_NAME, False),
            (gap, FG, False),
            (status, STATUS_COLORS.get(status, FG), True),
            (rest, FG, False),
        ]
    # Group headers ("zjtour (4)") and everything else.
    return [(ln, FG, False)]


def render(path, size):
    raw = open(path, encoding="utf-8", errors="replace").read()
    lines = [STRIP.sub("", ln).rstrip() for ln in raw.split("\n")]
    while lines and not lines[-1].strip():
        lines.pop()

    reg = ImageFont.truetype(FONT_REG, FONT_SIZE)
    bold = ImageFont.truetype(FONT_BOLD, FONT_SIZE)
    fb = ImageFont.truetype(FONT_FALLBACK, FONT_SIZE)
    cw = reg.getlength("M")
    ch = FONT_SIZE + 6

    img = Image.new("RGB", size, BG)
    d = ImageDraw.Draw(img)
    # A window bar, so the GIF reads as a terminal rather than a screenshot of
    # text. Drawn here rather than composited later: it is three circles.
    d.rectangle([0, 0, size[0], WINDOW_BAR], fill=(36, 40, 59))
    for i, col in enumerate(DOT_COLORS):
        cx = 20 + i * 20
        d.ellipse([cx - 6, WINDOW_BAR // 2 - 6, cx + 6, WINDOW_BAR // 2 + 6], fill=col)

    # Panel frames stay flush to the top, or a filter shrinking the list would
    # drift the view. Only the jump frame - the agent's pane, half as tall -
    # centres. Keyed on the header rather than height, for the same reason.
    y = WINDOW_BAR + PAD
    is_panel = any(ln.lstrip().startswith("zj-agent-mob") for ln in lines)
    if not is_panel:
        slack = (size[1] - WINDOW_BAR - PAD * 2) // ch - len(lines)
        if slack > 1:
            y += (slack // 2) * ch
    for ln in lines:
        # The selected row gets a band behind it, the way the panel highlights it.
        if ln.lstrip().startswith("▶"):
            d.rectangle([PAD - 6, y - 2, size[0] - PAD + 6, y + ch - 2], fill=SEL_BG)
        x = PAD
        for text, fg, is_bold in spans(ln):
            # Drawn one cell at a time rather than one span at a time. The panel
            # is a character grid, and advancing by `cw * len(span)` lets any
            # glyph whose drawn width differs from one cell shift everything
            # after it on the line - the columns stop lining up between rows,
            # which is immediately visible in a list view. Per-cell advance keeps
            # the grid exact and costs nothing at this frame count.
            for cell in text:
                if cell != " ":
                    if cell in BRAILLE_CHARS:
                        draw_braille(d, cell, x, y, cw, ch, fg)
                    elif cell in MEDIA_CHARS:
                        draw_media(d, cell, x, y, cw, ch, fg)
                    else:
                        f = fb if cell in FALLBACK_CHARS else (bold if is_bold else reg)
                        d.text((x, y), cell, font=f, fill=fg)
                x += cw
        y += ch
    return img


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    src, out = sys.argv[1], sys.argv[2]
    names = sorted(f for f in os.listdir(src) if f.endswith(".txt"))
    if not names:
        sys.exit(f"no frames in {src}")

    # One pass to find the widest/tallest frame, so every frame shares a canvas
    # and the GIF does not jitter as rows come and go.
    reg = ImageFont.truetype(FONT_REG, FONT_SIZE)
    cw, ch = reg.getlength("M"), FONT_SIZE + 6
    cols = rows = 0
    for n in names:
        body = open(os.path.join(src, n), encoding="utf-8", errors="replace").read()
        ls = [STRIP.sub("", x).rstrip() for x in body.split("\n")]
        while ls and not ls[-1].strip():
            ls.pop()
        cols = max(cols, max((len(x) for x in ls), default=0))
        rows = max(rows, len(ls))
    size = (int(cw * cols) + PAD * 2, int(ch * rows) + WINDOW_BAR + PAD * 2)

    frames, holds = [], []
    for n in names:
        frames.append(render(os.path.join(src, n), size))
        hold_file = os.path.join(src, n[:-4] + ".hold")
        hold = 1
        if os.path.exists(hold_file):
            try:
                hold = max(1, int(open(hold_file).read().strip()))
            except ValueError:
                pass
        holds.append(hold)

    # 10 fps: fast enough that typing reads as typing, slow enough that the GIF
    # stays small. Durations are per-frame, so a held frame is one frame with a
    # long duration rather than N copies - which is most of why this file is a
    # few hundred KB rather than a few MB.
    durations = [h * 100 for h in holds]

    # One palette for the whole GIF, from the frame with the widest vocabulary.
    # RGB frames get a private palette each: bigger, and the same colour can
    # quantise differently between frames, which shimmers.
    ref = max(frames, key=lambda f: len(f.getcolors(1 << 16) or [(0, 0)]))
    pal = ref.convert("P", palette=Image.ADAPTIVE, colors=128)
    quant = [f.quantize(palette=pal, dither=Image.NONE) for f in frames]
    quant[0].save(
        out, save_all=True, append_images=quant[1:], duration=durations,
        loop=0, optimize=True, disposal=1,
    )
    total = sum(durations) / 1000
    print(f"{out}: {len(frames)} frames, {total:.0f}s, {size[0]}x{size[1]}")


if __name__ == "__main__":
    main()
