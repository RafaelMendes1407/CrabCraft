# Bundled fonts

These fonts are distributed under the SIL Open Font License 1.1. Versions and copyright
statements are verified against the bundled font files’ metadata:

- JetBrains Mono (version 2.304) — © 2020 The JetBrains Mono Project Authors (https://github.com/JetBrains/JetBrainsMono)
- Inter (version 3.019, git-0a5106e0b) — © 2020 The Inter Project Authors (https://github.com/rsms/inter)

Complete license text: [SIL OFL 1.1](licenses/OFL-1.1.txt).


Additional bundled font:

- `NotoSansCJKsc-Regular.otf`, Noto Sans CJK SC Regular, version 2.004.
  Copyright © 2014–2021 Adobe (http://www.adobe.com/).
  SIL Open Font License 1.1; complete license in `NotoSansCJK-LICENSE.txt`.
  Unmodified upstream font from
  https://github.com/notofonts/noto-cjk/blob/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf
  (commit `f8d157532fbfaeda587e826d4cd5b21a49186f7c`).
  SHA-256: `2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b`.
  Embedded as fallback for Chinese and Japanese interface text and editor content, including
  bold/italic font families; it does not depend on a system font installation. The original
  font names, copyright and license metadata are preserved. Downloaded 2026-11-04.


## Default fallback fonts

CrabCraft retains the fonts embedded by `epaint_default_fonts` 0.36.2: Hack Regular,
Ubuntu Light, Noto Emoji Regular and emoji-icon-font. Their complete license texts are
in `licenses/`. See [THIRD_PARTY_NOTICES.md](../../THIRD_PARTY_NOTICES.md) for their
copyright, license and source inventory. System fonts discovered at runtime are not
redistributed. Binary packages must retain that notice, this inventory,
`NotoSansCJK-LICENSE.txt` and all files in `licenses/`.

## SHA-256 inventory

```text
42389d688e1d26230a27b0396944e616349301a5324eb2a6150b1c5a8b5abf67  Inter-Regular.otf
0f74c753f93f3f5ffa82f979445691f03ade49d4f6c295eea3f5db1c0849760d  Inter-SemiBold.otf
bcbffe83c1b4f9d5395ccb2dbe948948da0d4ab57b7950f53ce1dbcf9c51b74c  JetBrainsMono-Bold.ttf
4680cb953818d79d0e3019fc2dbac3f78b16d9194d6d56e16cb5b19650dc3602  JetBrainsMono-Italic.ttf
5a56708696f63922f56fb75d9fae69c8637c03bd306bb58001bde6e8c7506880  JetBrainsMono-Regular.ttf
2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b  NotoSansCJKsc-Regular.otf
```
