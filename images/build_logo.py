#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["cairosvg"]
# ///
"""Build images/logo.svg and images/logo.png.

Composes a circular badge holding a fanned deck of pull-request cards next to a
two-tone wordmark and tagline, then flattens the result through cairosvg so text
becomes glyph paths and the SVG renders identically on GitHub and crates.io
without any fonts installed. Requires the "Noto Sans" font when regenerating.
"""

import pathlib

import cairosvg

HERE = pathlib.Path(__file__).resolve().parent
OUT_SVG = HERE / "logo.svg"
OUT_PNG = HERE / "logo.png"

ACCENT = "#8250DF"  # GitHub's "merged" purple
ACCENT_SOFT = "#F3EEFC"
INK = "#3F3F46"
MUTED = "#8A8A8A"
OK = "#2DA44E"
WARN = "#BF8700"

# Cards fan out up and to the right: (x, y, fill). The last one is the front card.
CARDS = [(98, 74, "#DCD0F7"), (84, 90, "#EADFFB"), (70, 106, "#FFFFFF")]
CARD_W, CARD_H, CARD_R = 124, 82, 12

# Canvas width: the wordmark ends near x=870, leaving a right margin that matches the left.
WIDTH = 890


def card(x: int, y: int, fill: str) -> str:
    return (
        f'<rect x="{x}" y="{y}" width="{CARD_W}" height="{CARD_H}" rx="{CARD_R}" '
        f'fill="{fill}" stroke="{ACCENT}" stroke-width="3.5"/>'
    )


def front_card_details(x: int, y: int) -> str:
    """A tiny PR row: branch glyph, title/subtitle bars, review + CI pills."""
    gx, top, bottom = x + 20, y + 20, y + 62
    glyph = (
        f'<line x1="{gx}" y1="{top + 5}" x2="{gx}" y2="{bottom - 5}" stroke="{ACCENT}" stroke-width="3"/>'
        f'<circle cx="{gx}" cy="{top}" r="5" fill="#fff" stroke="{ACCENT}" stroke-width="3"/>'
        f'<circle cx="{gx}" cy="{bottom}" r="5" fill="#fff" stroke="{ACCENT}" stroke-width="3"/>'
    )
    tx = x + 42
    return (
        glyph
        + f'<rect x="{tx}" y="{y + 14}" width="66" height="9" rx="4.5" fill="{INK}"/>'
        + f'<rect x="{tx}" y="{y + 30}" width="46" height="7" rx="3.5" fill="#B4B4BC"/>'
        + f'<rect x="{tx}" y="{y + 50}" width="34" height="12" rx="6" fill="{OK}"/>'
        + f'<rect x="{tx + 40}" y="{y + 50}" width="26" height="12" rx="6" fill="{WARN}"/>'
    )


def build() -> str:
    cards = "\n    ".join(card(*c) for c in CARDS)
    fx, fy, _ = CARDS[-1]
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} 266" width="{WIDTH}" height="266" role="img" aria-label="pulldeck">
  <title>pulldeck</title>
  <desc>A fanned deck of pull-request cards — logo for pulldeck, a GitHub pull request tracker.</desc>
  <circle cx="133" cy="133" r="118" fill="{ACCENT_SOFT}" stroke="{ACCENT}" stroke-width="10"/>
  <g transform="translate(-13 2)">
    {cards}
    {front_card_details(fx, fy)}
  </g>
  <text x="290" y="150" font-family="Noto Sans" font-weight="700" font-size="140" letter-spacing="-2"><tspan fill="{INK}">pull</tspan><tspan fill="{ACCENT}">deck</tspan></text>
  <text x="296" y="224" font-family="Noto Sans" font-weight="400" font-size="34" letter-spacing="1" fill="{MUTED}">your pull requests, at a glance</text>
</svg>
"""


def main() -> None:
    source = build().encode("utf-8")
    flat = cairosvg.svg2svg(bytestring=source)
    OUT_SVG.write_bytes(flat)
    cairosvg.svg2png(bytestring=source, write_to=str(OUT_PNG), output_width=WIDTH * 2, background_color="white")
    print(f"wrote {OUT_SVG.name} ({OUT_SVG.stat().st_size} bytes) and {OUT_PNG.name}")


if __name__ == "__main__":
    main()
