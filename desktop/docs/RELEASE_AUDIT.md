# Release audit — 0.9.35

Audit date: 15 October 2026. Changes were developed in an isolated copy and promoted after verification. Personal music and the live application database were not modified by tests.

## Changes found by the audit

- Missing releases, Queue and Downloaded now share album artist → release → track browsing, lazy cached artwork, selection rules and context actions. Queue/Downloaded use a useful Status column instead of an empty recommendation column. Cached track names and ISRCs are searchable.
- Artist selection and keyboard selection respect AI exclusions. Synthetic group IDs never become download IDs. Metadata actions follow the online worker's state, independently of local scans.
- Queue approval batches roll back together if any chosen track is stale. Invalid request shapes return errors instead of a false success. Redownload accepts legacy numeric track IDs and changes the latest queue payload transactionally.
- Existing-download reuse verifies readable audio and recording identity, preserving conflicting files for review. Downloaded album-artist tags prefer current release metadata; explicit local replacement destinations retain their intended attribution.
- Cancellation interrupts download pauses promptly. Extreme pause settings are bounded safely, and the last track adds no unnecessary wait. Repeated credited-pagination pages stop with an error rather than making unnecessary requests.
- Correct tags handles MP4 integer fields without endless padding proposals. Invalid cached tag JSON is reread at startup. Disc-folder grouping is shared across tables, corrections and linking without mistaking release names for disc directories.
- Invalid folder templates remain visible as non-actionable review evidence. Removed an unused duplicate-removal implementation; tests now exercise the production reviewed-removal path.
- Download pause settings describe their actual sequential-track behaviour. Documentation and screenshots were refreshed; obsolete ignore rules and generated schema tracking were cleaned up.

## Coverage

Validation passed **32 frontend unit tests, 289 Rust tests, three FFmpeg checks and all 71 WebKit workflow tests**, plus the correctness lint and production frontend build. Twenty opt-in Rust checks are excluded from the ordinary suite; selected live tests and a real-audio roundtrip were run explicitly as described below.

The macOS 0.9.35 bundle built successfully and passed strict signature verification. Native startup, reopening its disposable database and the macOS appearance probe passed. Documentation screenshots were captured from the tested UI.

| Area | Evidence |
| --- | --- |
| Navigation, layout and appearance | All sidebar workflows; independent pane scrolling; Back/Forward/sidebar controls; native accent colours; Light/Dark; opaque table headers; dialogs and full-window screenshots. |
| Tables | Grouping, expansion, sorting, column filters, pagination, search, exact parent/child selection, context menus, off-page approvals, artwork visibility/cache, lazy track details and retry errors. |
| Local files | Real FLAC/M4A reads and writes; dates, number repair, key/lyric changes; folder moves; audio preservation; duplicate removal through a disposable Trash substitute; rollback and stale-review guards. |
| Scans and cache | Startup delta scans, malformed-cache recovery, unchanged-file reuse, review invalidation, route-independent jobs, disconnected libraries and shared metadata publication. |
| Linking and recommendations | Exact recording and sibling evidence, release structure, artist scopes, market availability, superseded releases, conservative credit/rights evidence, AI labels and ambiguous cases. |
| Queue and downloads | Atomic approval batches, exports, redownload, output collision protection, cancellation, parallel transfer rows, tag/ID publication, multi-copy MQA replacement and disconnected destinations. |
| Jobs and activity | Independent workers, conflict guards, progress/ETA, cancellation, saved history, filters, partial catalogue publication and refresh resume. |
| Account and online data | Mock sign-in prompt/cancellation/invalid redirects; existing subscriber session exercised live for searches, releases, metadata, credits, availability, favourites and downloads. |

### Live checks

Explicit bounded tests used the saved subscriber account copied into disposable databases, with token refresh disabled. They passed:

- Artist and track search, release lists, summary batches and unavailable IDs.
- Audio-only track positions, extended fields, credits, BPM/key and exact ISRC lookup.
- Market availability and favourite artists.
- Cached credited-release reuse with authentication subsequently disabled.
- Cold/warm catalogue jobs and cancellation/resume.
- Real FLAC download/redownload, decoding, tag verification and queue persistence.
- Real downloaded-file scan, linking, number correction, folder move, reviewed duplicate removal and multi-copy MQA replacement.
- Two concurrent tracks with independent progress rows while cached database reads remained responsive.

### Performance measurements

These are bounded samples on the development Mac, not a full-library speed guarantee:

| Check | Result |
| --- | --- |
| One artist's detailed recommendation refresh | Cold: 11.1 s / 55 requests. Warm: 0.32 s / 2 requests; no repeated detailed metadata fetches. |
| Two artists' release lists | Cold: 2.23 s / 7 requests. Warm: 1.35 s / 4 requests. |
| Saved credited metadata | Repeat load, including linking/download adapters, completed in 25 ms with authentication disabled and no network. |
| Three credited releases, 34 items | Serial: 1.18 s. Existing three-request concurrency: 0.91 s. Neither sample received HTTP 429. |
| Synthetic cache: 1,000 releases / 4,000 tracks | Serial reads: 9.81 s. Existing batched path: 2.98 s, in a debug build. |

The measurements support keeping bounded concurrency and improving reuse. No arbitrary increase in API pressure, speculative Rust rewrite or source minification was introduced. Normal production bundling remains unchanged.

## Reproduce and limits

Run `sh desktop/tools/check.sh` for the offline suite and `sh desktop/tools/check.sh --live` for the separate bounded download check. The [guide](DOCUMENTATION.md#development-and-verification) describes prerequisites and screenshot/build commands.

Automated tests cannot establish that every catalogue entry, storage device or account state works. Fresh interactive browser sign-in was not completed, to preserve the existing session. Native Finder/Trash interaction and the macOS quit-confirmation dialog still warrant a manual release check; their underlying paths, removal logic and job cancellation are covered separately. Browser tests use WebKit with the Rust RPC test bridge, not a full native-window automation driver. Live API results are a sample and can change upstream.
