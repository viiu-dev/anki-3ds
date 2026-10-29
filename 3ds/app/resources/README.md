# Application artwork

`icon.svg` is the Anki icon supplied by the project maintainer. It is retained
unchanged. `icon.png` is its 48×48 launcher version; `banner.png` uses the same
artwork. PNGs are committed so builds do not require an SVG renderer.

To regenerate the icon with CairoSVG 2.8.2:

```sh
cairosvg 3ds/app/resources/icon.svg -o 3ds/app/resources/icon.png -s 1.5
```

These assets do not imply endorsement by Ankitects.

`NotoSansJP.ttf` covers Latin, Cyrillic and Japanese. `NotoSans-Latin-Cyrillic.ttf`
is a Noto Sans subset with Ukrainian and other extended Cyrillic letters, used
as a per-glyph fallback. Both fonts are licensed under the SIL Open Font
License in `licenses/NotoSansJP-OFL.txt`.
