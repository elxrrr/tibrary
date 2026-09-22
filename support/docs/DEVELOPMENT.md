# Tibrary desktop development

## Run on macOS

The rebuilt app uses Tauri 2, React and a private Python 3.13 service. The packaged application includes Python, the audio/tag libraries, FFmpeg and both streaming adapters. End users do not need Node, Rust, Python or Qt installed.

1. Open `desktop/src-tauri/target/release/bundle/dmg/Tibrary_1.0.0-rc.1_aarch64.dmg`.
2. Drag **Tibrary** to Applications and open it. Alternatively double-click the repository's `Start.command` to open the local build.
3. Existing libraries, SQLite catalogue/link caches, Keychain credentials and account sessions retain their previous locations. The old Qt download destination and appearance are imported on first launch.
4. Use **Settings → Connections** to check the saved accounts; **Settings → Downloads** contains the destination, structure and streaming settings.

The local build is not Developer ID signed or notarized. Distribution outside this development machine requires signing/notarization with the publisher's Apple credentials. The current artifact targets Apple Silicon and macOS 13 or later; build on Intel for an Intel artifact.

For an isolated fictional library, run `support/tools/Demo.command`. It cannot apply file changes or download music. Do not run the archived Qt application against the same database concurrently.

## Build from source

Developer prerequisites: macOS, Xcode Command Line Tools, Node 22.12+ with npm, Rust stable with Cargo, and Python 3.12+ for the build environment. Upstream source checkouts must be present under `app/resources/` (including submodules when cloning).

```sh
python3 -m venv .venv
.venv/bin/python -m pip install uv==0.12.17
# The build installs the native engine and app into the private runtime.
.venv/bin/python support/tools/build_desktop.py
```

The build script installs a private, relocatable Python 3.13.15 distribution, installs the backend and upstream adapters, probes their interfaces without signing in, and builds the `.app` and `.dmg`. It never reads or writes music libraries. Generated runtime/package files are ignored by Git. `desktop/package-lock.json` and `desktop/src-tauri/Cargo.lock` pin frontend/shell dependencies. The package contains an installed-package inventory and upstream source/licences.

For frontend development:

```sh
npm --prefix desktop ci
cd desktop
npm run tauri dev
```

Install the native engine and service for development (Rust must be on PATH):

```sh
.venv/bin/uv pip install ./native/tibrary-tags -e ./app ./app/resources/python-tidal ./app/resources/tidaler
```

Development uses the repository `.venv`; production uses the packaged private runtime. Streaming component updates create checked builds under `~/Library/Application Support/Tibrary/streaming`, with rollback. The bundled runtime remains the fallback.

## Tests

```sh
PYTHONPATH=app:support/tests .venv/bin/python -m unittest test_desktop_service
npm --prefix desktop test
npm --prefix desktop exec -- playwright install webkit
npm --prefix desktop run test:e2e
```

WebKit tests communicate with a real Python service using disposable databases and temporary FLAC fixtures. The HTTP test interception exists only in the test build; production communication is private standard input/output, with no listening HTTP port.

For the retained backend and historical UI regression suite, install `PySide6>=6.8,<7` into the **development** environment only and run:

```sh
.venv/bin/python support/tests/run_isolated.py
```

That runner supplies the archived Qt module path, blocks external network requests, uses test credentials and rejects writes to external volumes. Qt is not a production dependency.

## Source map

| Location | Responsibility |
| --- | --- |
| `desktop/src/` | React interface, shared tables and selection state |
| `desktop/src-tauri/src/` | Native process supervision, dialogs, Finder/web integration, safe quitting |
| `app/library_manager/desktop_service.py` | Named operations, background job lifecycle, trusted previews and shared state |
| `app/library_manager/sidecar.py` | Private NDJSON request/event transport and database instance lock |
| `native/tibrary-tags/` | Lofty metadata I/O, padding-aware FLAC writes, cancellable audio hashing and MQA signal analysis |
| Remaining `app/library_manager/` modules | Scanning, linking, metadata, artwork, audit, consolidation, downloading and cache logic |
| `archive/qt/` | Previous Qt screens, retained for reference and regression tests |
| `support/tests/`, `desktop/tests/` | Backend and WebKit integration tests |

## Interaction and safety model

Pages read cached state and do not start scans when mounted. Startup performs one incremental library update in the Python job thread. Navigation does not cancel jobs. Each operation has one owner; duplicate starts and conflicting mutations are rejected. Activity is available in the header and Settings, without a permanent bottom status bar.

File changes require a server-held preview and explicit confirmation. The interface submits selected IDs, not writable tag/path payloads. Existing fingerprint, collision, audio verification and Trash safeguards remain in the Python core. Cancellation stops between safe file boundaries. Closing the window or choosing Quit requests cancellation and waits for the current safe operation before stopping Python.

Selection and expansion use stable release/track IDs. Sorting runs over complete cached datasets before pagination. Parent approval cascades to child audio tracks; partial selections persist in the acquisition queue. Statistics use the shared release-link query.

The architecture follows [Tauri's sidecar guidance](https://v2.tauri.app/develop/sidecar/) and [capability model](https://tauri.app/security/capabilities/), with loading feedback guided by [Apple's loading guidance](https://developer.apple.com/design/human-interface-guidelines/loading). Native file dialogs, system fonts and system light/dark appearance are retained; web tables provide consistent keyboard and pointer interaction.

## Native audio services

`native/tibrary-tags` is an in-process PyO3 extension (Python 3.12+ ABI), not a
per-file subprocess. Lofty reads the supported audio formats; FLAC comments and
MP4 atoms are edited in their native representations. It preserves repeated and
empty comments, credit order, custom DJ tags and embedded artwork. FLAC writes
reuse padding instead of shifting the audio unnecessarily. SHA-256 audio-frame
verification and the existing repeated-signal MQA detector also run in Rust.
I/O releases the Python GIL; integrity hashing checks cancellation between chunks.
Database transactions, provider calls, linking decisions, previews and publication
safeguards remain in the Python service.

The reviewed copy/verify/publish workflow remains authoritative. Native tag
writers must only receive staging files, never bypass a maintenance preview to
edit library originals. Leading legacy ID3 blocks in FLAC are rejected for writes
rather than stripped. Parsing failures leave the original files intact.

Upstream streaming sources stay independently updatable. The application installs
its own download-tag adapter in the isolated downloader process and supplies the
bundled ABI-compatible extension when that environment lacks it. Mutagen remains
an upstream dependency and a test oracle, not an application metadata backend.

Run the native contract tests with `test_native_tags` through the isolated runner.
`support/tools/benchmark_tags.py <library> --limit 100` compares readers without
changing source files; write measurements use temporary copies only. Benchmark
results do not imply faster network/API requests or already-cached scans.


### Shared scan and catalogue policy

The desktop service owns background operations independently of navigation. Startup
and **Refresh local files** discover filesystem changes; dependent pages use the
same persisted inspection, including empty libraries. A forced refresh walks once
and rereads tags. Normal refreshes reuse files with unchanged fingerprints. Applying
changes updates affected snapshots and the database; consolidation/repair refresh
once after completion. Changes made in another editor require a local refresh.

Every catalogue workflow uses `CachedTidal`: searches, recording editions and artist
summaries are shared across jobs and restarts, scoped to market (24 hours; empty
searches one hour). Explicit release-list refresh bypasses summary caching, while
unchanged track lists remain reusable. Release details use the configured cache
age (30 days by default); extended tags and availability use one day. Incomplete,
failed or cancelled fetches do not replace successful observations. The additional
metadata provider retains its existing market-scoped BPM/key cache.

Activity identifies the operation, library, cache reuse and reasons for online
fetches. Rapid routine cache/progress messages are throttled to keep the interface
responsive. The service runs one job at a time; route changes never start scans.

### Native filesystem boundary

`native/tibrary-tags/src/filesystem.rs` implements read-only inventory traversal
and staged FLAC copying with audio-frame hashing. `file_services.py` is the shared
Python boundary and retains filesystem audit events. Native operations release
Python's interpreter lock; cancellation callbacks run at bounded file/chunk
boundaries. Native copying never publishes, moves or deletes a library file.
The existing reviewed workflow owns publication and database reconciliation.

The native package is pinned by `app/pyproject.toml`. Rebuild its wheel before
running source changes that introduce native functions. The desktop builder
selects the exact native version declared in `native/tibrary-tags/pyproject.toml`.
