# Release verification — Tauri migration

## 22 September 2026

The supported desktop UI is now Tauri 2 + React, with the existing Python core behind a private stdio service. The former PySide6 UI is retained under `archive/qt/` and is absent from the packaged runtime.

### Passed

- Full isolated regression run: **316 tests**, including the retained backend algorithms and archived interface fixtures.
- Final focused run after migration refinements: **64 tests**, including **16 desktop-service integration tests**, download plumbing, cancellation, cache reuse, queue validation, preview ownership and metadata preservation.
- React selection state: **2 tests**.
- WebKit end-to-end: **6 scenarios**. All primary pages, local sorting/filtering, album/track approval cascades, expansion through sorting, selection persistence, batch ignore, dark settings/reset controls and partial-track queuing were exercised against a real Python service with temporary FLAC files and databases.
- Frontend TypeScript/Vite production build and Rust checks passed. npm dependency audit reports **0 known vulnerabilities**, including development dependencies.
- Packaged Python sidecar starts, answers protocol requests and exits cleanly. Native Tauri application launch and macOS Quit completed successfully.
- The private Python runtime has no Qt installation. Streaming adapter interface checks pass without signing in.
- Bundled FFmpeg extracts FLAC successfully; normalization preserves audio and existing BPM/Camelot key on a temporary audio fixture.
- A temporary copy of the live **19,394-track** database was used for read-only cache performance checks. Initial dashboard state: **0.70 seconds**; first unlinked table: **8.32 seconds**; repeated table: **0.001 seconds**. The table contained **775 unlinked tracks**. Results depend on cache size and hardware.
- The earlier 22.9-second synchronous summary calculation was moved out of state requests into the independent background worker, with persisted summary fingerprints for reuse.

### Artifacts

- `desktop/src-tauri/target/release/bundle/macos/Tibrary.app`
- `desktop/src-tauri/target/release/bundle/dmg/Tibrary_1.0.0-rc.1_aarch64.dmg`

The build contains Python 3.13.15, the application modules, streaming adapters, FFmpeg and upstream source/licence material. The installer targets Apple Silicon Macs on macOS 13 or later. Build commands and launch instructions are in [Development and setup](DEVELOPMENT.md).

### Boundaries

- No FLAC files on the NVME drive were written, moved or deleted. File mutation tests used disposable fixtures. Cache performance checks used a copied database.
- Routine API tests use mocked responses. No live-account end-to-end download or online component upgrade/rollback was performed during the Tauri migration; adapter and existing mocked workflow coverage are not a substitute for those live acceptance checks.
- This is a tested **release candidate**, not a signed public distribution. Developer ID signing, notarization and testing the distributed download on another clean Mac remain publication steps. Other operating systems have not been certified.
- No test suite establishes that every possible catalogue anomaly or user interaction is bug-free. Known regression cases are retained in tests rather than claiming perfection.


## Native services migration — 22 September 2026

Development and all mutation tests ran in the separate `tibrary-lofty-test`
checkout. The usable main build was retained until promotion. Rust now handles
Lofty metadata I/O, padding-aware FLAC writes, cancellable audio-frame hashing
and MQA signal analysis. Provider calls, linking, database transactions and
reviewed file publication remain in the established Python service.

- Native contracts cover FLAC, MP3, AAC/M4A, WAV and AIFF; an independent Mutagen
  reader checks the results. Tests preserve decoded audio, unknown application
  blocks, empty/repeated/custom tags, artist credit order, BPM, Camelot key,
  embedded artwork and MP4 freeform values. Padding growth/shrink, malformed
  files, cancellation, source protection and native write-audit hooks are covered.
- Full isolated regression suite: **334 passed**.
- Focused service/native regression run: **70 passed**. Packaged-runtime checks:
  **68 passed**, plus bundled FFmpeg extraction/normalization smoke testing.
- React selection: **2 passed**. WebKit application workflows: **6 passed**.
- Rust Clippy passes with warnings treated as errors. Native app startup and
  targeted macOS Quit pass using a disposable demo database. Separately managed
  provider runtimes can load the app's ABI-compatible native extension.
- A read-only sample of **100 NVME FLACs** matched the independent reader's tags
  and embedded pictures exactly. Warm-cache median read times over that sample:
  Mutagen **5.01 ms**, Lofty with pictures **4.17 ms**, Lofty scanning without
  pictures **2.46 ms**. These are parser measurements, not full-library/API speed.
  Five disposable-copy writes had medians of **0.282 ms** and **0.340 ms**;
  writing is not claimed to be faster. Network limits and cached scans are unchanged.
- The default Lofty writer's audio shifting was replaced with padding reuse;
  unrelated credit ordering and empty-value omission were explicitly addressed.
- No NVME originals, production database, link caches or credentials were changed.
  The previously usable app is retained in the test directory for rollback.

The local macOS bundle is now ad-hoc signed and passes strict deep signature verification before and after launch/quit. The launcher disables bytecode writes inside the installed app. Developer ID signing and notarization remain separate public-distribution steps.

## Native filesystem services and shared caching — 22 September 2026

Validated separately in `tibrary-layout-cache-test`, then promoted only after tests.

- Rust now also collects the file inventory (including nanosecond timestamps),
  skips symlinks, reports progress, and cooperatively cancels without reconciling
  missing files from an incomplete traversal. Shared scan/inspection workflows
  use that service. Index writes commit in batches of 64.
- Reviewed FLAC changes copy to the existing empty staging file and hash audio
  frames in one native pass. This eliminates the extra source-file hashing pass.
  Nonempty/symlink destinations are refused; source fingerprint, tag/artwork and
  audio verification still precede publication. Cancellation retains originals.
- **97 packaged-runtime tests passed**, covering native I/O, metadata preservation,
  cancellation, scans, shared caching, desktop services and download normalization.
  **2 React tests and 7 WebKit scenarios passed**. Rust Clippy treats warnings as
  errors. Build and streaming adapter compatibility checks passed.
- Controlled 2,500-file inventory: Python median **11.94 ms**, Rust **7.15 ms**.
  Initial scan with a stub metadata reader: previous **1,137.05 ms**, native
  inventory plus batched index **42.47 ms**. This isolates inventory/database
  overhead; it does not predict real audio parsing or online lookup speed.
- Live official-catalogue lookup succeeded in GB. Repeating the query was verified
  with network access disabled at the API boundary. A single approved track was
  downloaded to a disposable folder: **FLAC, 16-bit, 44.1 kHz**, embedded artwork,
  padded track number, queue marked downloaded. Subscriber metadata access worked;
  the first test track had no BPM/key supplied, which is not a connection failure.
  A second live lookup of a known DJ-metadata example returned **both BPM and key**;
  a fresh client reused those results without launching a network worker.
- Tests used temporary libraries/databases and a temporary copy of the subscriber
  session. The production library database and NVME FLACs were not modified.
- Native adapter version is **0.2.0**; builds select its exact versioned wheel,
  preventing an older wheel in the output directory from being packaged.
- The layout uses semantic macOS/WebKit system colours, isolated pane scrolling,
  a protected full-width drag region and padded native window controls. Update
  library follows Complete library. Cache rules are documented in DEVELOPMENT.md.

Public distribution remains pending Developer ID signing, Apple notarization and
installation testing on a separate clean Mac. No valid signing identity was
installed on the build Mac during this verification. The generated ad-hoc-signed
app/DMG is a tested release candidate, not a notarized public release.

## Recommendation and navigation refinement

- Recommendations now distinguish verified release artist credits from the name
  of the catalogue page. Strong recommendations require matching primary credits
  and copyright continuity, or label continuity across multiple local releases.
  Cached metadata for already-linked releases supplies missing local evidence.
  Other candidates stay inspectable; the Ignored filter bypasses timeline limits.
- Focused recommendation, hidden-item and navigation-during-preview checks passed,
  as did the page-rendering and pane-layout browser scenarios. A read-only check of
  57,899 cached releases completed in 8.32 seconds without fetching remote data.
  Recommendation badges express evidence strength, not certified authenticity.
- Overview shows linked/indexed tracks. The title area continues each pane's colour.
- Both obsolete migration test directories and generated compiler/build artifacts
  were removed after promotion and native launch/quit verification. Reusable test
  sources, archived UI source, runtime dependencies, databases and link caches remain.
  One previous app is retained at
  `~/Library/Application Support/Tibrary/build-backup/Tibrary-before-recommendations.app`.

## Contributor recommendations and overview (22 September 2026)

- Recommendation policy migrated to the native Rust module (0.2.1); Python retains
  database/evidence preparation and normalization, with no duplicate scoring policy.
- Contributor profiles combine fingerprint-valid local inspection tags and cached
  online track credits. Shared composers, lyricists, songwriters, producers,
  remixers and performers can support a release; the artist's own name is excluded
  from supporting contributor evidence. Distinct recordings and people are counted
  once. Verified primary credits remain required for strong recommendations, and
  compilations/conflicting credits do not become trusted anchors.
- Shared track/release caches are consumed without render-time network requests.
  Existing metadata lookups persist credits there for subsequent recommendations.
  Unavailable credits remain uncertainty; no bulk contributor crawl is triggered.
- Cached per-artist evidence is prepared once per view. A read-only production
  cache check covered 57,899 releases in 6.12 seconds; 2,965 had shared contributor
  evidence. Counts are coverage observations, not measured recommendation accuracy.
- Overview loads the newest 20 missing releases using the same cached table query,
  excluding owned, queued and ignored releases. Removed the unused recent-download
  state query. Initial dashboard navigation waits for library restoration and no
  longer requests a file table with an obsolete library path.
- Validation: native policy test, three recommendation integration checks, Clippy
  with warnings denied, frontend build, two WebKit workflow/startup checks and local
  table smoke test. All operations used fixtures or read-only cache access; no NVME
  music files were modified.
