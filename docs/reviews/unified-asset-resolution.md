# Unified asset resolution review

Scope: USD composition and scene-test discovery, Rhai source/import identity,
Modelica source admission and external resources, source-library stores, and UI
resource requests. The contract is in
[asset resolution](../architecture/56-asset-resolution-and-cache.md).

## Ownership and cutover

| Boundary | Authoritative mechanism |
| --- | --- |
| Path and URI algebra | `lunco-assets-path`: scheme grammar/case, canonical source addresses, traversal rules and standard URI component decoding. Literal internal filenames remain distinct from encoded URI components. |
| Native identity/storage | `lunco-storage`; `canonical_directory_root` admits shared native source roots. Standard file URIs retain native filename semantics. Unrepresentable UTF-8 source identities reject instead of narrowing. |
| Twin/cache context | `lunco-assets-core`: contributing-source anchors, prepared paths, mount identity and authored/cache precedence. Only absence advances a search. |
| Byte access | Registered asset sources plus `registered_read::AssetReadTarget`; the common bounded reader is consumed by USD preparation, scene validation and UI. |
| Installed source stores | `lunco-assets-runtime::library`: native/in-memory admission, containment, terminal I/O errors, immutable application identities and the registered `library://` reader. Its native mirror uses the same resolution. |
| USD discovery | The runtime projection and prepared-path context, including nearest Twin admission. Commit `9fcfb1ecc` repairs discovery without another resolver. |
| Rhai modules | `ScriptSources::canonical_id` is shared by dependency preparation and runtime resolution; runtime imports use prepared sources/ASTs without I/O. Twin ownership checks consume the shared scheme parser. |
| Modelica names | Rumoca supplies syntax/dependency/declaration facts; `SourceRootRegistry` maps unique admitted namespace owners and fences operations. No new import scanner was added. |
| Modelica resources | One URI interpretation and package/source/member projection. Qualified resource hierarchy remains distinct when packages share a `.mo` file. Byte reads use the common asset owner. |
| UI | One shared asynchronous byte-loader state machine, maintained native raster decoder and common resource preview/reload. Source/root retirement discards pending byte slots and replaces the decoder cache instance before widgets render. |

The language-specific code supplies semantics: USD's contributing-layer context,
Rhai's importing-source identity and suffix, and Modelica's qualified package and
storage facts. It does not implement another storage reader or cache search.
Modelica package declarations come from admitted parsed sources, including
packages absent from the palette. Invalid overrides do not fall back to the
cache; the UI displays admitted state without filesystem work during painting.

The clean cutover removes the Modelica image byte-loader/cache, synchronous
Bitmap byte/decode path and cache-directory URI joins. Installed-library class
storage mapping lives in `lunco-modelica-library::package_paths`. Common URI
scheme recognition also replaces the compiler's duplicate scanner. No public
compatibility aliases were retained.

The native reader review reproduced an escaped metadata-sidecar read. Engine
and installed-library sources, including the default directory source, now
share `NativeAssetReader`, which validates
payloads, metadata and directory operations before delegating streaming to
Bevy. Bevy's sidecar-path helper is private, so this adapter spells its `.meta`
sibling convention once; language consumers never interpret sidecars. The
filesystem containment check is not an atomic guarantee against concurrent
hostile filesystem replacement.

## Verification

The original source-store traversal failure was reproduced before changing the
owner. Focused conformance covers scheme normalization, literal/encoded names,
invalid URI components, native/in-memory source behavior, root aliases,
containment and symlink escapes, repeated source traversal, non-UTF-8 identity
rejection, package hierarchy/member rules, ambiguous owners and retirement.

Discovery production evidence: 171 identical catalog entries from checkout and
foreign working directories, the composed-reader conformance test, and six
authored Twin-search runtime assertions. The repaired discovery consumes the
same context as runtime; explicit `./` and `../` references remain layer-relative.

Native UI evidence is captured by `scripts/api/test_asset_resources.py`, with
observable scope/document assertions in
`assets/scenarios/tests/asset_resolution.rhai`. The driver owns an explicit API
port, verifies its process, records the live schema, captures resource views and
requires API Exit and port release. Captures include encoded names, minimal
native file URI syntax, missing/traversal diagnostics, source retirement, explicit
reload and URI reuse with replacement image bytes.
Two documents declaring the same package retain separate document-owned source
registrations: resolution reports ambiguity until one owner closes, then recovers
the remaining owner without reopening it.
Opening a Twin package in the editor retains its admitted directory owner.
Closing the document preserves the resource; replacing the Twin retires it.

Native Windows and browser runtime acceptance remain separate from Linux
compile/runtime evidence. Application source-store identity changes require an
application restart. Full class-document anchor scrolling is outside this asset
resolution cutover; resource fragments are parsed separately from filenames.

Hook review: identity, language-standard interpretation, containment, revision
fencing and I/O are owner mechanisms. `UriRegistry` retains language routing;
widgets dispatch typed resource/class actions. The shared preview supplies generic
load/diagnostic presentation and uses existing theme defaults. No Twin selection,
permission or scenario policy was added to Rust, and no new policy hook was needed.

Standards checked: [OpenUSD Ar](https://openusd.org/dev/api/ar_page_front.html),
the pinned Rust OpenUSD resolver/composition APIs,
[Rhai custom resolvers](https://rhai.rs/book/rust/modules/resolvers/custom.html),
[RFC 8089](https://www.rfc-editor.org/rfc/rfc8089.html),
and [Modelica 3.6 section 13.5](https://specification.modelica.org/maint/3.6/packages.html#external-resources).
LunCoSim supplies its own explicit resolver context; it deliberately does not
admit ambient working-directory lookup as Pixar's default filesystem policy does.

## Accepted checks (2026-10-16)

- Shared conformance: `cargo test -p lunco-assets-path -p lunco-assets-core -p lunco-assets-runtime -p lunco-modelica-ast -p lunco-modelica-source-roots --lib asset_resolution_contract -j 4` passed six cases. The strengthened native containment case also passed with a valid later cache present, proving terminal rejection rather than fallthrough.
- UI result seam: `cargo test -p lunco-assets-runtime --features ui --lib failed_result_slot_is_terminal -j 4` passed. Storage URI conformance: `cargo test -p lunco-storage --lib file_uris_preserve_native_paths_and_reject_invalid_addresses -j 4` passed after reproducing the minimal-file-URI rejection.
- `cargo check -p lunco-modelica-assets --bin build_modelica_library_assets -j 4` and the production `cargo build -p lunco-luncosim --bin luncosim -j 4` passed. The build reports existing dependency warnings from `parry3d-f64`; no new project warning was introduced.
- `scripts/api/test_asset_resources.py`: nine authored Rhai assertions, fourteen captures, exit 0. Directed inspection confirmed literal/encoded names, library pending-to-ready, file-URI loading and explicit reload, ambiguous-package recovery, document retirement, replacement bytes, admitted Twin view closure and Twin retirement.
- `scripts/api/test_path_interoperability.py`: 42 authored assertions, exit 0. The driver reuses editor-scene readiness and checks the current document-composition contract; native rename, negative target admission, save/readback and a USD-bound Modelica equation result passed.
- The production Twin-search gate reported `TESTS_OK 6`, exit 0. Repository and `/tmp` catalogue runs produced 171 identical entries.
- All controlled runtime launches used this checkout's production binary and owned API port 4286, with PID/cwd/executable verification. Every owned runtime exited and released its port. The skill catalogue validator passed all 43 entries.

Local evidence is under `.lunco/terrain-lines/full-review/` and
`.lunco/asset-resource-review/`. The new command's reference section was generated
from live GUI/headless schemas; unchanged feature-gated networking sections remain
in the canonical reference. These runs do not establish browser or native Windows
acceptance, nor full Modelica external-function/resource compatibility.
