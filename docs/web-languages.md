# Web language syntax support

The native editor detects HTML, CSS, JavaScript (including JSX, MJS and CJS),
TypeScript (including MTS and CTS) and TSX by extension. The background parser
supplies syntax highlights and folding ranges. HTML retains automatic closing
of tags and quoted attributes. Comment actions use each language's delimiters.

This is syntax support: no web language server is started. Semantic analysis,
completion, refactoring and JavaScript/CSS inside HTML script/style elements
are not implemented. JSX tags currently use plain syntax colors.

The queries in `crates/rc-syntax/queries/{html,css,javascript,typescript}.scm`
were written for CrabCraft. The linked parser dependencies use the MIT license:

- HTML: https://github.com/tree-sitter/tree-sitter-html (tree-sitter-html 0.23.2)
- CSS: https://github.com/tree-sitter/tree-sitter-css (tree-sitter-css 0.25.0)
- JavaScript: https://github.com/tree-sitter/tree-sitter-javascript (tree-sitter-javascript 0.25.0)
- TypeScript/TSX: https://github.com/tree-sitter/tree-sitter-typescript (tree-sitter-typescript 0.23.2)

The libraries retain their upstream license files in the Cargo packages. There
are no new proprietary assets or external language-server downloads.

Validation uses the source assets in `tests/fixtures/RustWeb`: a real Cargo
binary includes them with `include_str!`. The syntax tests parse the same files,
check representative colors and verify folding. Run:

```sh
cargo test -p rc-syntax
cargo run --offline --manifest-path tests/fixtures/RustWeb/Cargo.toml
```
