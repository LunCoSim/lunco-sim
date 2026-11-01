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

## Native resolver conformance

The native entry selector is `lunco-assets-core::resolve_native_asset`, with
root order supplied by `asset_read_roots`. Engine/Twin Bevy readers, synchronous
reads, directory consumers and dataset scans use that owner. Entry kind is
checked after the first existing entry is selected. Cache lookup advances only
on absence; a file cannot become a cached directory, and an invalid authored
dataset cannot reveal a cached product. Dataset installed state retains its
logical URI rather than an unconsumed filesystem path.

`lunco-assets-path` owns the scheme constants and preserves Twin/store authority
when anchoring relative references. Escaping `..` remains visible for admission
to reject. `TwinReadScope` captures an admitted root and namespace together;
synchronous USD/validation consumers reject foreign authorities before reading.
Native CLI input admission is owned by `resolve_asset_input`; its bare-input
search validates missing ancestors through storage before changing namespaces.

Native Windows paths remain `PathBuf` values through filesystem operations and
retain verbatim drive/UNC prefixes. Source discovery exports native UTF-8
spelling. Portable source identity is formed only after native root stripping;
standard file URIs are encoded and decoded by `lunco-storage`. Native browser
drags carry `PathBuf`, while registered-source drags carry logical addresses.
These are storage and identity invariants, so no Rhai policy hook is needed.

The edge-case audit reproduced a verbatim trailing-dot path selecting its
ordinary sibling, a scoped file URI reading foreign bytes, a Twin directory
predicate rejecting a regular file, and direct registered reads accepting invalid
relative syntax. An isolated-CWD test also reproduced an invalid native ancestor
selecting engine-library bytes; CLI admission now reuses storage ancestor checks
before advancing on an apparent absence. Storage decodes Windows file URIs to verbatim native roots;
`url` owns URI parsing and `std::path::Prefix` owns drive/share parsing. The
adapter supplies literal native filename semantics without another URI parser.
Encoding validates the original namespace before standard-library Win32
normalization, preserving rejection of malformed `//?/` addresses. URI decoding
preserves the original drive/share root even when a suffix looks like another
drive; native suffixes are copied without reparsing them as separate paths. Native CLI
inputs keep their host semantics; POSIX double-root input has a Unix CI case.
Scoped native reads enforce canonical Twin containment. Native/Twin directory
predicates distinguish files from lookup errors and honor file overlays. The
registered reader applies the shared relative grammar to payloads, metadata and
directory enumeration. These remain generic asset/storage invariants; no new
Rhai policy hook or dependency patch is needed.

Native conformance on Windows (2026-11-01): all 61 asset-core tests passed,
along with 11 URI-algebra tests, the dataset precedence regression, all 14 native
USD composition cases, four source-content closure cases and two storage
file-URI cases. The shared native CI gate runs on Windows, macOS and Linux;
macOS/Linux execution is not established by this local Windows run.

Nightly run `41320439024` passes Linux and Windows, including the Windows native
runtime gates. macOS builds the application and passes the preceding native
tests, then rejects the discovery test's invalid UTF-8 filename during fixture
creation (`EILSEQ`). The native byte-filename rejection fixture runs on Linux;
the common Unix discovery fixture still checks literal names, containment,
cycles and relative-root rejection. In-memory source admission also rejects
non-UTF-8 identities on every Unix host and unpaired UTF-16 surrogates on Windows
without requiring filesystem support. Both cases require the owner's
`InvalidInput` diagnostic. This separates portable source-identity rejection from
the native filesystem's representable filename set; runtime admission is unchanged.

Run `41329925142` confirms that both corrected source-admission fixtures pass on
macOS. It exposes a separate destination-identity gap: a Windows-only test
expectation and a non-Windows case-sensitive default disagree with the macOS
volume. `FilePathIdentity` now queries macOS `_PC_CASE_SENSITIVE` and compares
future components with Core Foundation's locale-independent case/canonical
Unicode comparison. The existing Windows ordinal comparison remains native;
no portable uppercase key is used for filesystem identity. A failed case query
still rejects ambiguous ownership with its diagnostic. The low-level test
compares missing-path snapshots against later filesystem lookups for ASCII case,
Unicode case and canonical Unicode equivalence, and checks unknown-case failure.
It runs before desktop packaging on each native runner; macOS also exercises a
temporary case-sensitive HFSX volume with the same built test binary. This is a storage
invariant consumed by dataset output admission; no Rhai selection policy or
third-party source change is involved.

The native query follows Apple's [`pathconf` usage](https://github.com/apple-oss-distributions/apache_mod_hfs_apple/blob/main/mod_hfs_apple2.c).
Core Foundation's [`nonliteral comparison`](https://developer.apple.com/documentation/corefoundation/cfstringcompareflags/kcfcomparenonliteral?language=objc)
compares canonical Unicode representations without ignoring diacritics.

Production Windows verification used `target/debug/luncosim.exe` built with
`cargo build -p lunco-luncosim --bin luncosim -j 4`:

- `luncosim test --scene scenes/tests/twin_search_paths/sim/scenes/search_paths.usda --api 4197 --threads 4 --readiness-timeout 180 --max-ticks 20000`: PASS, six authored assertions.
- The same bounded command for `scenes/tests/sandbox_smoke.usda`: PASS.
- `LUNCOSIM_API_PORT=4198 python scripts/api/test_path_interoperability.py`: PASS, 41 authored assertions on Windows, including negative rename cases and literal-path USD/Modelica persistence. The shared fixture wait compares native files with `Path.samefile`, including verbatim aliases.
- Rover Lab at `tutorials/sandbox/rover_lab.usda`, owned PID 4772/port 4197: readiness clear, `RoverLab_x2f_Rover_System` Running, no Modelica error. API Exit released the process and port.

Runtime logs are under `target/path-resolution-final-{twin,sandbox,api,demo}.log`
(the demo API snapshot is `target/path-resolution-final-demo-api.log`).
This checkout reports missing optional DejaVu font data and an empty installed
library index; neither prevented these source/runtime gates from passing.

Windows UX verification used the same production binary at 150% display scale.
`test_resource_preview_layout.py --uri-point 640 202 --text-point 640 360`
and `--compact --uri-point 480 112 --text-point 480 270` both exited 0.
Their 24 captures were inspected at 1920x1080 and 1440x810 physical pixels
(1280x720 and 960x540 logical window sizes). Missing-file diagnostics, four image
corners, title/close controls, full long-URI hover text, Copy URI, bounded text,
scroll/reload and image-after-text remained within the viewport. The long native
filename contains Unicode, spaces, `#`, `%` and `+` and exceeds 260 characters.
The driver takes measured logical hit positions and records them; screenshot
dimensions alone do not identify a widget or its input coordinates.

An additional owned compact session (PID 25760, port 4198, native window DPI 144)
clicked Copy URI and read back the Windows Unicode clipboard: the complete
402-character encoded URI matched exactly. Evidence is in
`target/path-ux-measured-{standard,compact}/` and `target/path-ux-clipboard/`.
All sessions exited through the API and released their ports, with clean render
logs. These input gestures exercise application events and clipboard output;
physical Explorer drag delivery, a fresh installer, macOS/Linux desktop UX and
browser acceptance were not exercised by this run.

### DRY recheck

Strict authored relative-path admission composes `relative_path` rather than
implementing a second volume/component parser. Colon and NUL components reject
at overlay admission as well as at the reader. Structured package reads and
compiler source discovery share `discovery::read_files_with_extension`; the
Modelica adapter only derives package-relative source identities. The engine,
Twin and installed-library reader adapters retain their distinct source/overlay
contexts while sharing native selection and storage admission. Browser HTTP
transport is target-specific and does not provide another Windows reader.

`models::tests::package_inventory_contract` creates a generic temporary package
and an escaped directory junction on Windows (symlink on Unix). The previous
package reader returned the outside source; the shared inventory rejects the
entire package with a diagnostic. The positive package retains its exact
relative file identities. The named test passed after the cutover; all 11
`lunco-assets-path` tests and the named asset-core
`unsafe_overlay_paths_are_not_stored_or_read` test also passed. These checks
are included in the desktop native-contract CI gate.

The separate final review also traced namespace selection through the compiler
and async engine. Both retain package read/inventory failures as terminal owner
diagnostics. Only an absent structured package permits namespace lookup in a
different source. Top-level model reads, package-marker probes and catalog
membership use the shared native selector, including the package root itself.
The junction contract also rejects an escaped top-level package/file entry and
root traversal. The compiler's malformed-UTF-8 package regression failed before
the terminal diagnostic was retained and passed afterward. The bounded final
unit command ran the package inventory contract plus all four named
`source_root_smoke::source_root_content_closure_*` cases successfully; logs are
in `target/path-rereview-deep-contracts.log`.
The same built package test also passed with its temporary directory beneath a
path containing spaces, Unicode, `#`, `%`, apostrophe and `&`; native junction
creation receives paths as environment values rather than shell command text.
That evidence is in `target/path-rereview-special-temp.log`.

The production binary rebuilt with
`cargo build -p lunco-luncosim --bin luncosim -j 4`. A separate runtime check set
`LUNCO_ASSET_ROOT=C:\Users\BELLATOR\AppData\Local\luncosim\current\assets`,
the installed source tree from the reported failure. Rover Lab (PID 26772,
port 4199) became ready with its rover system running. The installed sandbox
(PID 17092, port 4199) compiled and advanced all seven generated vehicle/thermal
systems without Modelica errors. Its full-scene readiness remains blocked by
two optional Python programs unavailable in this source binary; this is a
Modelica path gate, not full sandbox acceptance. Both owned sessions exited
and released their port. Snapshots and logs are under
`target/path-rereview-installed/`; build output is
`target/path-rereview-production-build.log`.

The final `LUNCOSIM_API_PORT=4198 python scripts/api/test_path_interoperability.py`
run passed 43 authored assertions (owned PID 27268). The catalog assertion requires
`LunCo/Actuation/SignedTorqueAllocator.mo` with its logical engine URI, exercising
the production package reader. Native rename, rejected targets, persistence and
USD-bound Modelica execution also passed. Evidence is in
`target/path-rereview-api.log`; the owned session exited and released port 4198.

The local installed executable is dated 2026-10-31 and predates these fixes.
Source commits and a checkout rebuild do not update that executable. Testing
the new binary against installed assets does not establish installer/update
acceptance.

## Catalog and package-browser boundary review (2026-11-01)

The catalog regression reproduced an outside directory link being published as
`twin://probe/escape/outside.rhai` (`target/path-catalog-before.log`). Native
catalog discovery now projects the same contained traversal used by complete
source inventories. Extension and private-directory filters are explicit inputs;
escape, repeated identity and I/O diagnostics reject publication. Startup,
packaging and scene-test discovery consume the scanner's `Result`. Both live
and indexed catalogs reject filenames that cannot become logical identities;
native separators convert without aliasing literal Unix backslashes.

The package browser now projects admitted `LibrarySource` membership and reads
through its existing reader. Modelica's `package_paths` remains the file-layout
owner. Parsing validates the requested declaration, including its `within`
namespace; corrupt bytes and missing members remain terminal diagnostics.
The UI shows a wrapped error and explicit Retry. Scan requests carry only a node
identity and resolve its authoritative package path through the current tree.
Package nodes no longer carry an unused native filesystem field. The browser's
parsed-bundle adapter implements the same success/error interface.

DRY discovery found the existing contained inventory, library byte reader and
Modelica file mapper; these were extended or composed without another resolver,
dependency, fork or policy hook. Containment and storage identity are generic
owner invariants. Native paths remain `PathBuf` at the filesystem boundary;
logical source/class identities cross UI and compiler boundaries.

Windows verification:

- Four focused contracts passed: catalog traversal, structured package inventory,
  native/in-memory store resolution and package-tree parsing. Logs are
  `target/path-catalog-contracts.log` and `target/path-catalog-final-tests.log`.
  Store membership fixtures use the admitted source base, preserving Windows
  canonical native namespace semantics.
- `cargo build -p lunco-luncosim -p lunco-assets-runtime --bin luncosim --bin build_asset_manifest -j 4`
  passed without warnings (`target/path-catalog-production-build.log`). The
  packager accepted a Unicode/space/`#`/`%` fixture and rejected an outside
  junction with exit 1 while preserving the existing manifest
  (`target/path-catalog-packager.log`).
- Owned production PID 27108, port 4212: seven reviewed captures show the narrow
  package browser, a corrupt-byte diagnostic, retained failure after file repair,
  explicit Retry recovery and opening the correct read-only class. Native DPI was
  measured at 144 before converting captured coordinates to logical input.
  Three authored Rhai assertions passed (`target/path-package-browser-verdict.log`).
  Captures and runtime logs are under `target/package-browser-review/500f1e5a-c647-4ed6-87ec-408ec22456e0/`.
- `LUNCOSIM_API_PORT=4213 python scripts/api/test_path_interoperability.py`
  passed 43 authored assertions with owned PID 20912, including the reported
  allocator's logical catalog address and USD-bound Modelica execution
  (`target/path-catalog-interoperability.log`). Both owned runtimes exited through
  the API and released their ports. The skill validator passed all 43 entries.

Local Windows evidence does not establish macOS/Linux or browser execution;
the native CI gate includes these mechanism contracts on all three desktop hosts.
The installed 2026-10-31 executable still requires a new packaged release.

## External conformance review

Microsoft's namespace rules and Rust's pinned standard-library implementation
confirm that verbatim native paths must retain their semantics. RFC 8089 and the
maintained `url` API supply file-URI syntax and encoding. `dunce` simplifies only
when conversion is unambiguous; `normpath` distinguishes normalization from
filesystem canonicalization. LunCoSim retains native paths and requires canonical
containment at source admission, so a second normalization dependency would not
replace the existing resolver or its ownership checks.

The URI codec rejects native dot-directory components when their filesystem
meaning cannot be preserved by URI lexical normalization. Ordinary Windows input
first uses the standard library's Win32 normalization; verbatim paths are never
silently rewritten. Decoded separators cannot introduce hidden dot traversal.
Unix native parent traversal must be admitted through the filesystem owner
before URI encoding. The corresponding Unix cases run in the native CI gate.

Sources: [Microsoft namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file),
[Rust absolute paths](https://doc.rust-lang.org/std/path/fn.absolute.html),
[RFC 8089](https://www.rfc-editor.org/rfc/rfc8089),
[url file conversion](https://docs.rs/url/2.5.8/url/struct.Url.html#method.to_file_path),
[dunce](https://docs.rs/dunce/latest/dunce/), and
[normpath](https://docs.rs/normpath/latest/normpath/trait.PathExt.html).
RFC 9844 updates scoped IPv6 UI entry and does not change this native drive/share
conversion contract.

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

Native rename handling relocates directory registrations and all source/package
provenance, including saved facts, pending saves and closed-source facts.
Pre-rename worker operations cannot publish. Disk roots reload through the
existing worker after the rename's automatic class/save transaction. The focused
production driver is `scripts/api/test_asset_resource_renames.py`; its authored
checks live in `asset_resolution.rhai`, and its captures cover open/closed folder
moves, file rename/save/close/reopen, retired declarations and missing resources.

Platform runtime evidence is recorded separately; native Windows conformance
does not establish macOS, Linux or browser application acceptance. Application source-store identity changes require an
application restart. Full class-document anchor scrolling is outside this asset
resolution cutover; resource fragments are parsed separately from filenames.

Hook review: identity, language-standard interpretation, containment, revision
fencing and I/O are owner mechanisms. `UriRegistry` retains language routing;
widgets dispatch typed resource/class actions. The shared preview supplies generic
load/diagnostic presentation and uses existing theme defaults. No Twin selection,
permission or scenario policy was added to Rust, and no new policy hook was needed.
The workbench owns viewport containment and transient scroll reset through egui's
public widget/state APIs.

Standards checked: [Windows native namespaces](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file),
[Rust native absolute paths](https://doc.rust-lang.org/std/path/fn.absolute.html),
[OpenUSD Ar](https://openusd.org/dev/api/ar_page_front.html),
the pinned Rust OpenUSD resolver/composition APIs,
[Rhai custom resolvers](https://rhai.rs/book/rust/modules/resolvers/custom.html),
[RFC 8089](https://www.rfc-editor.org/rfc/rfc8089.html),
and [Modelica 3.6 section 13.5](https://specification.modelica.org/maint/3.6/packages.html#external-resources).
LunCoSim supplies its own explicit context for logical asset reads. Native CLI
input admission has the working-directory search defined by the shared asset
contract; language adapters do not add ambient filesystem searches.

## Accepted checks (2026-10-16)

- `cargo test -p lunco-storage -p lunco-assets-core -p lunco-assets-runtime -p lunco-modelica-ast -p lunco-modelica-source-roots --lib asset_resolution_contract -j 4`: eight passing cases. Coverage includes regular-file admission, streaming and seek semantics, shared overlay ownership, complete native/memory membership, traversal diagnostics, package membership/provenance and save fencing against older admission results.
- `cargo build -p lunco-luncosim --bin luncosim -j 4`: passed. Existing vendored `parry3d-f64` warnings remain; no new native project warning was introduced.
- Browser storage: `cargo test -p lunco-storage --target wasm32-unknown-unknown --lib opfs_stream_budget_and_cancelled_read_seek -j 4` passed in headless Chrome with the matching wasm-bindgen runner and ChromeDriver. One test covers demand-sized slices, byte limits and seeking after a cancelled pending read.
- `cargo check -p lunco-assets-runtime --target wasm32-unknown-unknown -j 4`: passed, including the registered Twin OPFS adapter and generic library store. Existing warnings in unchanged platform/dependency code remain. This is not full browser-application acceptance.
- `scripts/api/test_asset_resources.py`: 22 authored Rhai assertions, 28 captured and inspected resource views, exit 0. Positive images, explicit replacement/reload, multiple top-level declarations, ambiguous owners, invalid/current parse transitions, sibling-file membership, edited declarations, saved and unsaved closure, and Twin retirement passed. A FIFO was rejected before opening. The 70 MiB Twin payload used 67,108,937 process-read bytes including API/screenshot transport activity under the 64 MiB payload budget; both Twin and native file requests displayed the size diagnostic.
- `scripts/api/test_path_interoperability.py`: 42 authored assertions, exit 0, including native rename/case/rejected targets, save/readback and USD-bound Modelica execution. The production Twin-search gate passed six authored assertions and exited 0.
- `cargo build -p lunco-assets-runtime --bin build_asset_manifest -j 4`: passed. The executable generated the authoritative catalog for a source/data fixture, excluded its hidden source and rejected a missing directory with exit 1. `bash -n scripts/build_web.sh` passed.
- All controlled simulator launches used this checkout's production binary and owned API port 4286, with PID/cwd/executable verification. Every owned runtime exited and released its port. The skill catalogue validator passed all 43 entries.
- Rename follow-up: `cargo test -p lunco-modelica-source-roots --lib asset_resolution_contract -j 4` passed three cases, including relocation, saved facts and stale-operation rejection. The production binary rebuilt successfully. `scripts/api/test_asset_resource_renames.py` passed 20 authored assertions and exited 0; ten reviewed captures show resource recovery and the expected retired/missing diagnostics. The owned session released port 4286. Evidence is in `.lunco/asset-resolution-rereview/fixed/` and its adjacent `rename-*` logs. This verifies resource loading; preview layout is covered separately below.
- Preview layout: the production build passed, and `scripts/api/test_resource_preview_layout.py` completed both standard 1280x720 and compact 960x540 runs with exit 0. All 24 directed captures were reviewed: image corner markers, title/close controls, long-URI tooltip and copy menu, bounded text notice, scroll/reload and image-after-text stayed within the viewport. The copy menu was inspected; OS clipboard delivery was not exercised. Both owned API sessions on port 4286 exited with clean render logs. Evidence is in `.lunco/resource-preview-layout/complete-standard/` and `complete-compact/`. The driver records captures and lifecycle; visual review supplies the layout verdict.
- Local-main integration: the merged production build and ten focused discovery tests passed. The path driver passed 43 authored assertions; resource renames passed 20 assertions with all ten captures reviewed. The sunrise gate passed nine checks and the sun-disc occlusion gate passed three GPU frames. Owned API sessions used port 4286 and exited cleanly. Evidence is in `.lunco/usd-main-integration/` and the corresponding `target/scene-tests/` logs.

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

## Open TODO

Bound browser HTTP response allocation before consumer limits, as specified in
the [shared resource contract](../architecture/56-asset-resolution-and-cache.md#source-libraries-and-resource-views).
Acceptance requires an oversized HTTP payload in a real browser; the OPFS test
covers a different backend.
