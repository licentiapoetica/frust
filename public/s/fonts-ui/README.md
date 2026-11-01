# UI fonts

Small fonts used by the site's own CSS for specific UI elements. Unlike `fonts-builtin/`, these are
not offered as website fonts in Settings.

| Font | File | Source | License |
|---|---|---|---|
| f0ckm Nav Arrows | `NavArrows.woff2` | Noto Sans Mono 2.014, Copyright 2022 The Noto Project Authors (https://github.com/notofonts/latin-greek-cyrillic) | SIL Open Font License 1.1, [`LICENSE-Noto-OFL.txt`](LICENSE-Noto-OFL.txt) |

`NavArrows.woff2` holds only ← (U+2190) and → (U+2192), for the item page's "← prev" / "next →"
(`.steuerung-arrow` in `f0ckm.css`). Manrope, the site font, has no arrows, so the browser used to
draw them in whatever fallback font the system had, at a different height on every system. Shipping
the glyphs makes them the same everywhere. The outlines are moved up 104 units (of 1000) so the
arrow's ink centre sits at 0.27em, the middle of Manrope's x-height, so it is centred on lowercase
text with no CSS offset. It is a renamed Modified Version as the OFL requires; Noto has no Reserved
Font Name.
