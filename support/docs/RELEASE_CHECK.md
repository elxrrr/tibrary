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
