# Syntax highlighting feasibility for authored text

Date: 2026-08-30
Scope: tutorials checkout; research only; no Rust, dependency, or production
highlighting changes.

## Decision

Do not add a syntax-highlighting dependency or production implementation as
part of this pass. The safe future shape is one workbench highlighter that
returns an `egui::text::LayoutJob`, selected by file language and consumed by
both read-only and editable text views. A failed or unsupported parse must
fall back to the existing monospace `TextEdit` without changing editing,
selection, copying, or source loading behavior.

The implementation should be a later, separately approved work item because it
changes the core GUI binary and its dependency/grammar supply chain. It is not
an authored-asset-only fix.

Disposition: keep this review as the recorded research decision; no production
syntax-highlighting change is included in the tutorials commit.

## Current editor boundary

The generic source viewer in
`crates/lunco-workbench/src/source_viewer.rs` currently routes `rhai`,
`btxml`, `wgsl`, `usda`, `usd`, and `usdc` to a plain monospace `TextEdit`.
It does not route `.html` or `.css`, even though HUI uses those extensions.
The Modelica editor has its own document buffer, selection, debounce, and
diagnostic flow; it is not a drop-in replacement for the generic source
viewer. `egui_commonmark` is already optional in the Modelica UI for rendering
documentation code blocks, but that does not provide a shared source-editor
contract.

`TextEdit::layouter` is the narrow integration point. It can keep the existing
editor interaction while replacing only the galley construction with a
`LayoutJob`. Because the layouter may be called repeatedly while a widget is
visible, highlighting must be memoized by at least source generation/hash,
language, theme, and wrap width. Full parsing or grammar loading must not run
on the UI thread for every frame.

## Feasibility matrix

| Format family | Existing/available option | Recommendation | Main risk |
|---|---|---|---|
| USD / USDA / USDC | No canonical Tree-sitter grammar was found in the maintained parser list. USD has nested blocks, asset paths, metadata, relationships, and domain-specific `inputs:` names. | Start later with a small lexer producing conservative colors for comments, strings, asset paths, numeric literals, prim types, and common declaration keywords. Keep unknown tokens neutral. | A generic C-like grammar would miscolor USD composition and make authored facts look like semantics it does not understand. USDC is binary and should remain an inspector/read-only path, not text-highlighted source. |
| Rhai | No maintained canonical Rhai grammar was identified in the reviewed Tree-sitter sources. | Start later with the same conservative lexer family as USD, covering comments, strings, numbers, keywords, function calls, and identifiers. Do not use highlighting as validation. | Rhai syntax evolves with the embedded engine and the source may be incomplete while being edited. A parser-backed editor would need an explicit version/grammar owner. |
| Modelica | OpenModelica maintains `tree-sitter-modelica`, including a Modelica 3.5 grammar and highlighting queries. The repository still requires generated parser/build tooling and carries its own OSMC license file. | Candidate for a later parser-backed implementation, but first compare its tokens with the already authoritative Rumoca syntax cache and keep highlighting separate from compile/diagnostic state. | Two parser paths can drift. Adding a second parser to the core UI increases native build, grammar, licensing, and maintenance surface. |
| BCCP / HUI HTML | `tree-sitter-html` is a maintained MIT-licensed grammar. The current source-view extension list does not include `.html`. | Add extension routing only as part of a future shared highlighter. HTML structure and HUI custom elements can use the HTML grammar, with unknown HUI attributes left neutral. | HTML reload semantics are already owned by HUI. A source viewer must not imply that syntax highlighting validates or reloads a template. |
| BCCP / HUI CSS | `tree-sitter-css` is a maintained MIT-licensed grammar with generated parser artifacts. | Pair with HTML only in the future shared editor; preserve plain text for imported or unsupported stylesheet dialects. | CSS custom properties and Flair-specific selectors are project vocabulary, not evidence that a generic grammar understands the runtime style contract. |
| WGSL / shaders | `gpuweb/tree-sitter-wgsl` mirrors the upstream WGSL grammar, but its README requires parser build generation. | Prefer a later parser-backed or conservative lexer implementation after measuring grammar size and generated-artifact policy. Keep shader compilation and highlighting independent. | Build-tool/native parser cost and grammar freshness. Highlighting must never hide WGSL compiler errors or alter shader hot reload. |

## Candidate integrations

### `egui::LayoutJob` with a small shared lexer

This is the lowest-risk first implementation. It adds no grammar runtime and
can cover the project-specific USD and Rhai forms that generic grammars do not
model. The lexer should be deliberately non-semantic: comments and quoted
strings first, then numbers and a small language keyword table, with all other
text rendered using the theme's normal source color. It should emit spans, not
rewrite source, and it should be usable by read-only generated files as well as
editable buffers.

Estimated effort: small-to-medium for a read-only viewer; medium when shared
with the Modelica editor because selection, cursor position, wrapping, and
diagnostic jumps must retain their existing ownership.

### `syntect`

Syntect is a practical option for common text formats and already appears in
the dependency graph through the optional documentation-rendering path. It
would still need an explicit integration in the workbench, a grammar/theme
policy, and a decision about shipping the relevant syntax definitions. It is
not a free way to cover USD, Rhai, or project-specific HUI vocabulary, and
making it a direct workbench dependency expands the core GUI build.

Estimated effort: medium for common formats; medium-to-large for the complete
five-family scope and a stable theme/fallback contract.

### Tree-sitter

Tree-sitter provides incremental parsing and can reuse an edited previous tree,
which is attractive for an editable Modelica or WGSL editor. The reviewed
grammar coverage is uneven: HTML, CSS, and Modelica have credible candidates;
WGSL has an upstream-aligned grammar but requires generated parser setup; USD
and Rhai still need a project-owned strategy. Each grammar is another native
parser artifact and release/ABI/license surface.

Estimated effort: large for a shared multi-language editor; appropriate only
after selecting a small supported language set and defining grammar ownership.

## Runtime and UX contract for a future implementation

1. Detect language from the existing source path, with explicit `.usda` / `.usd`
   / `.usdc` distinctions and `.html` / `.css` routing added deliberately.
2. Produce a `LayoutJob` off the hot path and cache it. Invalidate on source
   generation, theme change, language change, or width change as required by
   wrapping.
3. Preserve the current `TextEdit` as the interaction owner. Highlighting is a
   presentation layer and cannot own source mutation, save, reload, diagnostics,
   or compile state.
4. Keep incomplete/invalid source renderable. Parser errors produce partial
   spans or plain text; they do not block loading, editing, HUI reload, or WGSL
   reload.
5. Bound work for long files. Prefer a visible-range or chunked strategy after
   measurement; never perform an unbounded full-file parse on every frame.
6. Use `lunco-theme` tokens rather than hardcoded colors and test light/dark
   contrast, selection readability, and generated/read-only source.

## Go / no-go boundary

Go for a separately approved implementation when the shared highlighter API,
supported language set, dependency licenses, grammar update policy, and
long-file budget are agreed. The first slice should be one read-only source
viewer with a plain-text fallback and focused visual checks.

No-go for this task: adding `tree-sitter`, `syntect`, a new parser crate,
editor-side semantic validation, or production highlighting. None is required
to improve authored sky, rover, antenna, or plume assets, and each would
require a Rust core rebuild.

## References

- [egui `TextEdit`](https://docs.rs/egui/latest/egui/widgets/text_edit/struct.TextEdit.html) and [`LayoutJob`](https://docs.rs/egui/latest/egui/text/struct.LayoutJob.html)
- [`egui_extras` syntax highlighting](https://docs.rs/egui_extras/latest/egui_extras/syntax_highlighting/index.html)
- [`syntect`](https://docs.rs/syntect/latest/syntect/)
- [Tree-sitter parser API and incremental editing](https://github.com/tree-sitter/tree-sitter/blob/master/lib/include/tree_sitter/api.h)
- [Tree-sitter parser list](https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers)
- [`tree-sitter-html`](https://github.com/tree-sitter/tree-sitter-html) and [`tree-sitter-css`](https://github.com/tree-sitter/tree-sitter-css)
- [`OpenModelica/tree-sitter-modelica`](https://github.com/OpenModelica/tree-sitter-modelica)
- [`gpuweb/tree-sitter-wgsl`](https://github.com/gpuweb/tree-sitter-wgsl)
