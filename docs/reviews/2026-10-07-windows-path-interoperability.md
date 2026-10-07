# Windows path interoperability

The path fixes use the existing asset, storage, document, and manifest owners.
The `usd` branch was fast-forwarded to `main` at
`804c5414415562cc112aca559f66f70ceaac34da`, and the task changes were restored
and reconciled with its bounded preparation, projection, native mass properties,
and prepared-solver content identity mechanisms.

## Implemented contracts

| Boundary | Current contract |
| --- | --- |
| Dataset output ownership | Scan workers prepare `lunco-storage::FilePathIdentity` for source and output destinations. Admission compares canonical ancestors and Windows directory case semantics without I/O. Overlapping outputs or sources are rejected. |
| Asset loads and handle lookup | Consumers use `lunco-assets-core::asset_path::load_asset_path` and typed Bevy paths. Literal `#`, `%`, spaces, and Unicode remain filename characters; intentional labels use explicit typed fields. This includes scripting, timelines, catalogs, shaders, runtime UI, SysML, and Modelica/Python sources. |
| Native USD references | Windows drive-absolute, UNC, and verbatim references enter the existing native file URI contract. Drive-relative references fail. Foreign Windows filesystem references fail explicitly on other hosts. Logical engine and Twin references retain their portable meaning. |
| Twin rename | Incoming roots and ordinary sources use canonical filesystem identity. Open workspace and Modelica document origins follow the rename. Case-only renames of ordinary entries are admitted when the destination identifies the same source. Distinct existing entries and symbolic-link targets cannot be overwritten. |
| Portable materialization | Dataset destinations and processing outputs reject Windows devices, reserved punctuation, trailing dots/spaces, and traversal. Scenario manifests additionally reject case aliases, duplicates, and file/directory conflicts before publication or client lifecycle replacement. |
| Shared downloads | The shared destination owner validates every scope before policy evaluation or writes. Shared engine destinations obey the same containment and portability rule as Twin destinations. |
| Modelica warm directories | `LUNCOSIM_WARM_DIRS` uses `std::env::split_paths`: semicolons on Windows and colons on Unix. Drive letters remain intact. |
| Asset root failure | Library root resolution and dependent native accessors/builders return errors. Invalid explicit configuration exits with a diagnostic; it does not panic or discover another library. Scheme handlers propagate resolution errors. |
| Modelica root directories | The Twin directory resolver accepts the manifest's `.` designator for the admitted root. File references remain concrete child addresses, and parent traversal remains rejected. |
| Program format selection | The shared USD program reader preserves literal filename characters for named/native sources. Only HTTP URLs use query/fragment separators when selecting an executor. |
| Document stage identity | Readers use the stage ID committed by the document projection owner. They do not rediscover it through a filename shared by different asset types. |
| Root-owned flat Modelica sources | The compiler reuses an admitted definition when the source's exact CID belongs to the root content closure. A conflicting definition is rejected instead of replacing the root. |

The native identity query uses the maintained Windows bindings for
`FileCaseSensitiveInfo` and ordinal comparison. Metadata handles request
`FILE_READ_ATTRIBUTES`, and an unavailable case mode rejects ambiguous
ownership. The directory flag is defined by the
[Windows SDK contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_case_sensitive_information).
Identity snapshots do not lock out external filesystem changes.

The path fixes do not rewrite shipped USD layers or Modelica equations. The
incoming `main` revision's `RoverDrivetrain.mo` correction is preserved.
Static inventory
found 816 tracked asset files, 105 Modelica sources, and six dataset manifests
with eight entries: no nonportable filename components, case-alias components,
or invalid declared destination/output paths. Runtime scratch USD documents
are authored only through `NewDocument`, `InspectUsdDocument`, typed
`ApplyUsdOp`, and `SaveAsDocument`, then opened and read back through the app.

## Verification

Focused low-level evidence:

- `cargo test -p lunco-storage -p lunco-assets-path -p lunco-networking-scenario -j 4 --lib`: 41 tests passed.
- Seven focused owner tests passed for asset-root and scheme diagnostics,
  dataset output overlap, all download scopes, and portable scripting names.
- The URL source-pool regression passed; invalid URL basenames require an
  explicit portable destination instead of an invented filename.
- Three existing asset-owner tests passed for logical/source/native filenames.
- The Twin root-directory regression passed, including parent traversal,
  concrete-file rejection, and retired mount ownership.
- The pure source-format regression passed for literal named/native filenames
  and HTTP path/query/fragment classification.
- The compiler regression passed for exact admitted flat-source reuse and
  rejection of a conflicting definition. These commands cover 55 low-level
  tests in total.
- Windows MSVC compilation of the storage/path owners and their tests passed:
  `cargo check -p lunco-storage -p lunco-assets-path --target x86_64-pc-windows-msvc --tests -j 4`.
- The `lunica` wasm build check passed before integration, exercising its typed
  Modelica deep-link load. Existing unrelated dead-code warnings were reported.
- Native caller/test/example checks and the rename owner's test compile passed.

The production binary was built with `networking,sysml` after integration.
The literal-filename Twin search gate passed all six assertions at tick 30
(`target/windows-interop-search-main-final.log`). GUI and headless schemas were
captured from owned sessions and the command reference regenerated from their
273-command union.
The final owned windowed session on port 4193 passed 42 authored assertions:
canonical-root and case-only rename, rejected destinations and symlink overwrite,
Modelica origin/source updates, save/reopen, literal USD save/readback, typed edit
promotion to the canonical stage, and the model's equation result of `2`.
Its scratch Twin is `target/path-interoperability/17e87bff-a5b2-4187-81a3-f5beac195ce5/`;
the driver log is `/tmp/windows-interoperability-live-main-final.log`.
The final production build and focused compiler test passed without new warnings.
All owned GUI/headless/schema/test sessions exited and released their API ports;
other sessions were left alone.
Logs are task-owned files under this checkout's `target/`; native compile and
unit logs are under `/tmp/windows-interoperability-*.log`.

## Evidence limits

Linux execution does not prove Windows-native filesystem behavior. The nightly
Windows job now runs the focused native path tests and the literal-filename
production scene gate. Windows CI and a native Windows windowed rename/model
save session remain required acceptance evidence.

The scripting asset gate reads one shared inventory of the current prelude and
policy sources. Readiness is a policy asset, and terrain sensing belongs to the
sensing prelude. The Modelica drive-law gate acquires manual control before
writing throttle and releases the same authority after its verdict; its motion
threshold remains unchanged.

Twin package installation publishes parsed namespace identities with the exact
scoped source-set operation. The registry retains these identities on the owned
entry, resolves compile dependencies to a unique source-set owner, and rejects
missing or ambiguous namespace ownership. Stale results cannot restore names
removed by Twin close. The existing path-interoperability Rhai assertions cover
a qualified package wrapper's solved output and terminal unknown-namespace
rejection. Focused compile and production evidence for these changes remains
pending.
