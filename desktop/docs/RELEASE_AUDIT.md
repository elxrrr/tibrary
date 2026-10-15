# Release audit — 0.9.37

Audit date: 15 October 2026. Changes were developed in an isolated copy and promoted after verification. Personal music and the live application database were not modified by tests.

## Discovery, playlists and artist images — 0.9.37

- Added artist portraits and playlist artwork to the shared visible-only thumbnail cache. Requests are bounded and successful images persist on disk.
- Added Discover, New releases and Playlists tabs. New releases now contains the latest missing music from the selected library alongside separate catalogue picks; Overview keeps its counts and library controls.
- Added editorial, created and saved playlists, playlist search/link opening, paginated audio tracks and cancellable loading of longer lists. The track radio action offers provider recommendations without presenting editorial lists as personal recommendations.
- Playlist selections retain their original album IDs. Up to three release checks run concurrently before one atomic queue transaction; failures leave the selected batch unqueued. Repeated playlist positions remain visible but queue each recording once.
- Account-scoped page caches, market checks, unavailable-release handling and AI exclusions remain in place. Browsing does not add artists to library discovery.
- Fixed the shared Select-all checkbox accepting clicks while its table was empty or loading. Updated documentation and screenshots in both themes.

Validation passed **36 frontend unit tests, 306 Rust tests and three FFmpeg checks**, plus the correctness lint and production frontend build. All **87 functional WebKit workflows** and the documentation capture passed across the full run and focused repair checks. The full run exposed the Select-all loading race and an overly broad network-error fixture; after repair, both cases passed three consecutive runs. The repeated-playlist-position regression also passed. Twenty-two Rust tests remain opt-in.

The bounded live discovery check passed in **6.03 seconds**: editorial home, playlist search/details/pagination, created/saved lists, related tracks, actual artist/playlist images, exact cross-release queueing and disconnected cache reuse. The separate browser-to-download test passed in **4.55 seconds**, verifying a real decoded FLAC, tags and queue persistence. Both used copied credentials with refresh disabled and disposable databases/output.

The final 0.9.37 macOS bundle passed strict signature verification, native startup, reopening its disposable database and all four native appearance palettes. The final playlist browser checks and the moved library-recommendation filtering assertions passed against the rebuilt backend. Existing links, account settings and catalogue caches are preserved; no reset is required.

## Browse music — 0.9.36

- Added an independent artwork-led catalogue browser: live home sections, artist/release/track search, paginated artist discographies, release track selection and catalogue-link opening. Back/Forward preserves results and scroll position.
- Browse uses the subscriber account and shared metadata/artwork caches. It does not add browsed artists to library discovery or change local links. Page caches have bounded retention and market/account separation; stale results remain labelled and cannot authorize downloads.
- Full releases and selected tracks merge into the reviewed download queue transactionally. Known-unavailable and AI-labelled music is excluded. Browse remains usable without a local library and while independent jobs run.
- Fixed a download-completion race: adding tracks or changing a queue decision during a download no longer lets the older batch mark the newer selection complete.
- Fixed Activity history loading additional pages from an obsolete intersection notification. Pagination now waits for committed rows and checks the current scroll position.
- Added focused browser/backend tests, fresh screenshots and concise usage documentation. The native macOS bundle is version 0.9.36.

Validation passed **34 frontend unit tests, 303 Rust tests and three FFmpeg checks**, the correctness lint and the production frontend build. All **81 functional WebKit workflows** passed across the full run and focused repair checks: the full run found one Activity pagination race; after fixing it, the four affected cases passed three consecutive runs, including a deterministic stale-observer regression. The separate Browse screenshot capture also passed. Twenty-one opt-in Rust tests remain excluded from the ordinary suite.

The live browser test used the saved subscriber account in a disposable database: home sections, all three search types, a paginated artist release list, catalogue-link resolution, credited audio tracks, an exact single-track queue selection and a real FLAC download. It verified decoding, release/track tags and the terminal queue state. Cassie's edition with 13 catalogue items correctly exposed 12 audio tracks. Cached browsing worked after disconnecting the test account, and browsing did not expand the local artist catalogue. The final live sample passed in 4.50 seconds; this is a bounded check, not a whole-library performance guarantee.

The final 0.9.36 app passed strict signature verification, native startup, disposable database reopening and the macOS appearance probe. Dark/Light layouts, the new Browse screenshots and the existing documentation gallery were inspected. No live music files were changed.

## Previous core audit — 0.9.35

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
