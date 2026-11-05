# Third-party asset notices

CrabCraft embeds the open-source fonts and icons listed below. Their licenses apply to those
assets; the application's [MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) licensing does not
replace them. Binary packages must retain this file, the referenced asset notices and license
texts. No proprietary RustRover assets are included.

## Fonts supplied by CrabCraft

| Font files | Copyright retained in the font metadata | License and source |
| --- | --- | --- |
| Inter Regular and SemiBold, version 3.019 | Copyright © 2020 The Inter Project Authors | [SIL OFL 1.1](assets/fonts/licenses/OFL-1.1.txt); [Inter source project](https://github.com/rsms/inter) |
| JetBrains Mono Regular, Bold and Italic, version 2.304 | Copyright 2020 The JetBrains Mono Project Authors | [SIL OFL 1.1](assets/fonts/licenses/OFL-1.1.txt); [JetBrains Mono source project](https://github.com/JetBrains/JetBrainsMono) |
| Noto Sans CJK SC Regular, version 2.004 | © 2014–2021 Adobe (http://www.adobe.com/) | [SIL OFL 1.1](assets/fonts/NotoSansCJK-LICENSE.txt); [exact upstream file](https://github.com/notofonts/noto-cjk/blob/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf) |

See [the font inventory](assets/fonts/README.md) for the embedded filenames, versions and
SHA-256 checksums. Copyright, font-name and license metadata remain in the font files.

## Fonts supplied through egui

CrabCraft retains egui's default font definitions as fallback fonts. Cargo.lock selects
`epaint_default_fonts` 0.36.2, whose bundled font files and license texts are available in the
[upstream crate source](https://docs.rs/crate/epaint_default_fonts/0.36.2/source/fonts/).
These font assets have their own licenses, independently of the egui Rust code.

| Font | Copyright / attribution | License and source |
| --- | --- | --- |
| Hack Regular 3.003 | Copyright 2018 Source Foundry Authors; Copyright 2003 Bitstream, Inc.; DejaVu contributions committed to the public domain | [MIT and Bitstream Vera terms](assets/fonts/licenses/Hack-Regular.txt); [Hack source](https://github.com/source-foundry/Hack) |
| Ubuntu Light 0.83 | Copyright 2011 Canonical Ltd. | [Ubuntu Font Licence 1.0](assets/fonts/licenses/Ubuntu-UFL-1.0.txt); [Ubuntu font project](https://design.ubuntu.com/font) |
| Noto Emoji Regular 1.05 | Copyright 2013 Google Inc. All Rights Reserved. | [SIL OFL 1.1](assets/fonts/licenses/OFL-1.1.txt); [Noto Emoji source](https://github.com/googlefonts/noto-emoji) |
| emoji-icon-font 1.1 | Copyright (c) 2014 John Slegers | [MIT license](assets/fonts/licenses/EmojiIcon-MIT.txt); [source project](https://github.com/jslegers/emoji-icon-font) |

The license texts above are copied from the locked crate's `fonts` directory, except the
generic OFL text, which is identical to the existing Noto Sans CJK license text in this
repository. OS fonts discovered at runtime are not redistributed in CrabCraft packages.

## Icons

The open-source IntelliJ Platform icons are copyright 2000–2025 JetBrains s.r.o. and
contributors, licensed under Apache-2.0. The Rust/Cargo file icons come from the archived
open-source IntelliJ Rust plugin, under its MIT license. The complete Rust-plugin copyright
and permission notice, upstream locations and pinned Rust-plugin source commit are retained
in [assets/icons/NOTICE.md](assets/icons/NOTICE.md). Keep that notice with binary packages.

The Rust logo is a trademark of the Rust Foundation and is used to identify Rust files.
These source attributions do not imply affiliation with or endorsement by the upstream
authors, JetBrains, Canonical, Google, Adobe or the Rust Foundation.

## Application lineage

CrabCraft derives from [DotCraft](https://github.com/javy-fernandez/dotcraft). Its original
copyright and license notices remain in the repository and in the application's license
files. This asset inventory does not replace the license metadata of Rust dependencies.
