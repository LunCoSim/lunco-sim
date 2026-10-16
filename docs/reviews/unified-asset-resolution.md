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
payloads, metadata and directory operations. The shared storage opener admits
regular files. Owned handles supply demand-sized reads and ordinary seek
semantics on the asset I/O pool, without independent read-ahead. Bevy's
sidecar-path helper is private, so this adapter spells its `.meta`
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
Per-file facts refresh with the current parsed document generation without
retiring directory ownership. Separate class files keep their package membership.
Save commits facts; close discards unsaved overlays and preserves saved resources.
Older in-flight admission cannot overwrite a save. Replacing the Twin retires it.

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

- `cargo test -p lunco-storage -p lunco-assets-core -p lunco-assets-runtime -p lunco-modelica-ast -p lunco-modelica-source-roots --lib asset_resolution_contract -j 4`: eight passing cases. Coverage includes regular-file admission, streaming and seek semantics, shared overlay ownership, complete native/memory membership, traversal diagnostics, package membership/provenance and save fencing against older admission results.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed. Existing vendored `parry3d-f64` warnings remain; no new native project warning was introduced.
- Browser storage: `cargo test -p lunco-storage --target wasm32-unknown-unknown --lib opfs_stream_budget_and_cancelled_read_seek -j 4` passed in headless Chrome with the matching wasm-bindgen runner and ChromeDriver. One test covers demand-sized slices, byte limits and seeking after a cancelled pending read.
- `cargo check -p lunco-assets-runtime --target wasm32-unknown-unknown -j 4`: passed, including the registered Twin OPFS adapter and generic library store. Existing warnings in unchanged platform/dependency code remain. This is not full browser-application acceptance.
- `scripts/api/test_asset_resources.py`: 22 authored Rhai assertions, 28 captured and inspected resource views, exit 0. Positive images, explicit replacement/reload, multiple top-level declarations, ambiguous owners, invalid/current parse transitions, sibling-file membership, edited declarations, saved and unsaved closure, and Twin retirement passed. A FIFO was rejected before opening. The 70 MiB Twin payload used 67,108,937 process-read bytes including API/screenshot transport activity under the 64 MiB payload budget; both Twin and native file requests displayed the size diagnostic.
- `scripts/api/test_path_interoperability.py`: 42 authored assertions, exit 0, including native rename/case/rejected targets, save/readback and USD-bound Modelica execution. The production Twin-search gate passed six authored assertions and exited 0.
- `cargo build -p lunco-assets-runtime --bin build_asset_manifest -j 4`: passed. The executable generated the authoritative catalog for a source/data fixture, excluded its hidden source and rejected a missing directory with exit 1. `bash -n scripts/build_web.sh` passed.
- All controlled simulator launches used this checkout's production binary and owned API port 4286, with PID/cwd/executable verification. Every owned runtime exited and released its port. The skill catalogue validator passed all 43 entries.

Current evidence is under `.lunco/asset-resolution-fix/` and the `fix-*` logs in
`.lunco/asset-resolution-audit/`. These runs do not establish native Windows or
full browser-application acceptance, nor full Modelica external-function/resource
compatibility.

## Shared boundary regression coverage

Native and memory source stores use complete file inventories; Modelica owns
source extension filtering. The common traversal reports unreadable, escaped,
repeated and unrepresentable identities instead of silently dropping entries.
Post-startup native installation prepares the inventory on its existing worker.
The web manifest generator is invoked from its owning `lunco-assets-runtime`
package.

Storage admits only regular files for reads. Twin/native sources open streaming
handles before the common consumer budget applies; overlays pin immutable bytes
without cloning them. Browser OPFS uses bounded on-demand slices and keeps its
JavaScript snapshot in a local task, with explicit offsets for cancelled reads
followed by seeking. The source-fact bridge carries typed file provenance through
the compiler, worker, registry and resource resolver.

The resource production driver covers edited and retired declarations,
separate-file member rejection, save/close persistence, unsaved-close restoration,
invalid-parse recovery, FIFO rejection before open and a 70 MiB Twin payload
rejected by the 64 MiB UI budget. Linux process-I/O evidence distinguishes a
bounded stream from eager full-file materialization. Browser storage tests cover
bounded reads and seeking after a cancelled read. The accepted checks above
record the production results.
