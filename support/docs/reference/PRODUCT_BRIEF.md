> Historical product brief. For current behaviour, see the [user guide](../README.md) and [linking architecture](LINKING_AND_UI_AUDIT.md).

# TIDAL Library Manager — Product and Technical Brief

Status: initial product brief  
Date: 15 September 2026

## 1. Product summary

A local-first desktop application that understands an existing music library, links each local artist and release to the correct TIDAL catalogue entities, identifies releases or tracks that appear to be missing, and lets the user review a download plan before exporting it or handing approved items to Tidaler.

The same application will also expose the existing local tag repair, date cleanup, stray-track matching, and folder-organization functions through a polished graphical interface.

The guiding principle is **scan once, review clearly, change only with approval**. Catalogue discovery and comparison may be automatic; downloads, tag edits, moves, and deletions must remain explicit actions with previews and auditable results.

## 2. Problem to solve

The user has a large local collection originally sourced largely from TIDAL, but:

- local tags are inconsistent or incomplete;
- artist names alone are not enough to identify the correct TIDAL artist;
- some releases were downloaded and others were missed;
- keeping up with new releases manually is burdensome;
- there is no unified view of local holdings, TIDAL catalogue coverage, proposed downloads, and local cleanup actions.

The application should answer four questions:

1. What music is actually present locally?
2. Which TIDAL artist, album, and track does each local item correspond to?
3. What relevant catalogue items are missing locally, including historical gaps and newer releases?
4. What should happen next: ignore, mark owned, queue, export, download, retag, or reorganize?

## 3. Product boundaries

### In scope

- macOS-first local desktop application, designed so Windows support remains practical.
- One or more local library roots, including disconnected/offline drives.
- Incremental metadata scanning and a persistent SQLite catalogue.
- FLAC, M4A/MP4/ALAC, MP3, AIFF, and WAV support matching the current organizer.
- Artist, release, and track resolution against TIDAL.
- Confidence-scored candidate matching with manual override.
- Catalogue comparison and a persistent review queue.
- Export to a durable interchange format and optional Tidaler hand-off.
- Existing tag/folder maintenance features with preview, confirmation, backups, and logs.
- Clear handling of unavailable, region-restricted, duplicate, deluxe, remastered, compilation, and various-artists releases.

### Not in the first release

- A replacement music player or DJ performance application.
- Automatic downloading without a reviewed queue.
- Cloud-hosted storage of the local library catalogue.
- Acoustic fingerprinting as a mandatory dependency. It can be added later for difficult matches.
- Claiming every item on an artist page is desirable. Appears-on compilations, videos, clean/explicit variants, remasters, and duplicates need policy controls.

## 4. Primary user journey

### A. Add and scan a library

The user chooses a library root. The app inventories supported audio files and reads tags in a background worker. Each file is fingerprinted cheaply using path, size, and high-resolution modification time; unchanged files reuse cached metadata. New, changed, moved, and missing files are reconciled incrementally.

The dashboard immediately remains usable with the last saved snapshot, even if the music drive is disconnected. It shows scan progress, last successful scan, staleness, errors, and the number of files that require attention.

### B. Resolve local music to TIDAL

The app first looks for durable identifiers already embedded in tags or paths: TIDAL IDs/URLs, UPC/barcode, ISRC, album ID, or track ID. It then searches using progressively weaker evidence:

1. exact identifier;
2. album title + album artist + year + track count;
3. several track titles from the same local release + artist;
4. normalized artist name + release-title overlap;
5. artist-name search alone, only as a candidate generator.

An artist candidate receives a confidence score based on how many independently matching local albums and tracks appear on that TIDAL artist page. A shared name is never treated as a confident match merely because it is the top search result.

The user sees an **Artist Inbox** containing:

- confident automatic matches;
- ambiguous candidates needing confirmation;
- unmatched local artists;
- evidence for every suggestion;
- a permanent manual link/unlink option.

Confirmed mappings are stored by TIDAL ID, not display name, and reused on later scans.

### C. Build a local-versus-TIDAL coverage view

For every confirmed artist, the app fetches and caches the artist's albums and EP/singles, then resolves their track lists. Local files are compared at both release and track level.

Release dates help sort the timeline and define convenient views, but are not the identity rule. Reissues and deluxe editions often have later dates while substantially duplicating an owned release. The comparison should classify catalogue items as:

- **Owned complete** — all relevant tracks matched locally;
- **Owned partial** — some tracks are missing;
- **Missing release** — no adequate local match;
- **Alternate edition** — materially overlaps an owned release;
- **Ignored** — user chose not to acquire it;
- **Unavailable** — present in metadata but not currently playable in the user's region;
- **Needs review** — evidence is ambiguous.

The principal filters are:

- gaps between the two newest locally owned releases;
- everything newer than the newest owned release;
- all historical gaps;
- partial albums;
- newly discovered since the last catalogue refresh;
- singles/EPs versus albums;
- include/exclude compilations, live releases, remixes, deluxe/remasters, clean/explicit variants, and videos.

### D. Review and fulfil missing music

The **Acquisition Queue** is a persistent checklist, not an ephemeral search result. Each row shows artwork, artist, title, release type/date, missing track count, quality/availability when known, reason it was suggested, and a direct TIDAL link.

Actions:

- select/deselect individual tracks or whole releases;
- mark as already owned and manually link to a local file/release;
- ignore once or ignore permanently;
- choose a preferred edition when duplicates exist;
- export approved TIDAL URLs/IDs as text, CSV, JSON, or M3U-style list;
- send approved items to Tidaler;
- view queued, running, completed, skipped, and failed items;
- rescan the destination after completion and verify that expected tracks arrived.

For the first releasable version, file export is the lowest-risk integration. Direct Tidaler integration follows once a stable internal boundary is added, ideally invoking a supported service/adapter rather than importing GUI internals. The app must not scrape Tidaler's visual interface.

### E. Maintain tags and folders

The **Library Tools** area incorporates the current organizer's functions:

- normalize the album-artist tag;
- match stray/loner tracks;
- normalize date tags;
- organize into `Artist/Album (Year)/Track - Title`, with multi-disc folders;
- browse saved libraries and inspect all non-artwork tags;
- preview every proposed change before applying it;
- optionally back up affected files;
- rescan and reconcile the database after changes.

The current organizer can permanently delete destination-name collisions and `cover.jpg` files. In the new UI, destructive operations must be separately disclosed, disabled by default, and shown item-by-item in the preview. Prefer moving suspected duplicates and removed covers to a recoverable quarantine/trash location until the user explicitly purges them.

## 5. Interface concept

A native-feeling dark desktop UI with five primary destinations:

1. **Overview** — library health, scan/catalogue status, unresolved artists, new discoveries, queue summary, recent activity.
2. **Artists** — local artists, confirmed TIDAL identity, match confidence, evidence, owned/missing counts, release timeline.
3. **Missing Music** — filterable comparison table and edition-resolution tools.
4. **Acquisition Queue** — checkbox review, export/download hand-off, progress, retry, and verification.
5. **Library Tools** — tag and folder cleanup previews, backups, execution, and history.

Important interaction details:

- Never freeze the interface during drive or network work.
- Preserve filter, sort, selection, and scroll state.
- Use progressive results: show cached data first, then refresh it.
- Make confidence explainable (for example, “3 local albums and 21 track titles match”).
- Every mutating action has preview → confirmation → execution → result.
- Provide keyboard-friendly bulk review, but no ambiguous one-click “fix everything.”

## 6. Matching and gap-detection rules

### Normalization

Create comparison-only normalized values without overwriting original tags:

- Unicode normalization and case folding;
- whitespace and punctuation normalization;
- configurable removal of edition suffixes such as deluxe, remastered, expanded, radio edit, and explicit/clean markers;
- featured-artist parsing that preserves the original credit;
- disc/track-number normalization;
- date precision awareness (year-only is weaker than a full date).

### Scoring

Use deterministic, inspectable scoring rather than an opaque model. A possible initial weighting:

- exact TIDAL ID: conclusive;
- exact UPC or ISRC: very strong;
- multiple matching tracks within the same album: strong;
- album title + primary artist + compatible date: strong;
- duration within a small tolerance: supporting evidence;
- track count and disc structure: supporting evidence;
- artist name alone: weak.

Store both the selected mapping and all material evidence. Thresholds should produce three states: auto-accepted, review required, and unmatched. Manual decisions always take precedence and survive rescans.

### Edition policy

The user should be able to choose a default policy, then override it per artist or release:

- prefer original release or newest/highest-quality edition;
- treat a deluxe edition as satisfied when all standard tracks are owned, or require bonus tracks;
- collapse clean/explicit duplicates or prefer explicit;
- ignore releases where the artist is not primary;
- ignore tracks already owned on another release, or preserve release completeness.

This policy determines the actionable missing list. The app should still expose the raw catalogue comparison for audit.

## 7. Proposed architecture

Use a modular Python 3.12+ desktop application. PySide6 is the pragmatic initial UI choice because Tidaler already uses it, while the existing Textual organizer logic can be separated from its terminal presentation and reused.

Core modules:

- **Scanner** — incremental filesystem inventory and Mutagen tag extraction.
- **Local catalogue** — SQLite schema, migrations, queries, cached artwork thumbnails, and scan history.
- **TIDAL catalogue adapter** — OAuth, search, artist/release/track retrieval, pagination, availability, retry/backoff, and response cache.
- **Resolver** — normalized entities, candidate generation, evidence scoring, and manual overrides.
- **Coverage engine** — owned/partial/missing/alternate classification and policy evaluation.
- **Queue service** — persistent desired state, export, Tidaler adapter, progress, retry, cancellation, and post-download verification.
- **Organizer service** — previewable tag/date/folder plans and safe execution, extracted from the current project.
- **Desktop UI** — presentation only; long operations run outside the UI thread and report progress through signals/events.

Avoid tightly coupling the application to python-tidal's internal object model. Define a small catalogue interface and map responses into our own records. This lets the official TIDAL API serve discovery while Tidaler/python-tidal remains an optional fulfilment adapter.

## 8. Data model additions

Retain and migrate the existing `libraries`, `files`, and `tags` data. Add at least:

- `scan_runs` and `file_scan_state` for incremental history and errors;
- `local_artists`, `local_releases`, and `local_tracks` as normalized projections;
- `tidal_artists`, `tidal_releases`, and `tidal_tracks` with raw-response/version metadata;
- `artist_mappings`, `release_mappings`, and `track_mappings` with confidence, status, evidence, and provenance;
- `catalogue_syncs` with market, fetched time, expiry, and error state;
- `coverage_results` with policy version and reason;
- `queue_items`, `queue_runs`, and `queue_events`;
- `ignore_rules` and edition preferences;
- `operation_plans`, `operation_results`, and backup/quarantine references.

Store secrets outside SQLite. Use the operating system credential store for refresh tokens/client secrets where possible. Never log tokens, client secrets, stream URLs, or decryption material.

## 9. TIDAL and Tidaler integration strategy

The official TIDAL API is suitable for authenticated catalogue discovery: search, artists, albums, tracks, relationships, availability/usage information, and artwork. OAuth client credentials can cover catalogue-only access; user-context authorization should be added only if a required feature needs it. The developer secret must not be embedded in a distributable desktop binary, so an initial personal/local build can use local secure configuration while wider distribution will need an appropriate public-client/PKCE design or a small credential broker approved by TIDAL.

Tidaler is an unofficial downloader that uses python-tidal and accepts TIDAL URLs. Keep its responsibilities isolated:

- our app decides what is missing and what the user approved;
- the adapter submits only approved stable TIDAL URLs/IDs;
- Tidaler handles media retrieval, format/quality, metadata, artwork, lyrics, and existing-file behavior;
- our app observes result events and verifies the library afterwards.

The user's paid TIDAL/DJ subscriptions may determine playback or catalogue entitlements, but they should not be assumed to grant unrestricted permanent-download rights or to remove API/application policy constraints. The implementation should respect TIDAL's API terms, regional availability, usage rules, and applicable law. This is a product/compliance checkpoint before distribution, even if a private local tool is technically functional.

## 10. Performance and resilience

- Read tags only for new or changed files; use path, size, and `mtime_ns` as the initial invalidation key.
- Detect probable moves using a lightweight content signature or tag/size evidence before declaring delete + add.
- Keep artwork out of the tag catalogue; cache resized thumbnails separately and lazily.
- Batch database writes and API requests, respecting endpoint pagination and rate limits.
- Cache TIDAL responses with per-entity timestamps and allow manual refresh.
- Retain the last good catalogue snapshot when refreshes fail.
- Make scan, sync, matching, and queue jobs cancellable and resumable.
- Use WAL mode and explicit schema migrations; regularly back up the small application database.
- Never rely on a network connection for browsing already scanned local data.

## 11. Delivery plan

### Phase 0 — technical proof (small, disposable UI)

- Authenticate to the official TIDAL API without putting credentials in source control.
- Resolve a deliberately small sample of 8–12 releases chosen to cover the highest-risk cases: one clean match, one same-name artist, one badly tagged release, one single/EP, one deluxe or remaster, one compilation/featured appearance, one partial album, and one unavailable or regional item where available.
- Fetch complete album and EP/single discographies with pagination.
- Prove URL export and a manual Tidaler import/run.
- Measure false matches and API behavior before committing to thresholds.

Exit criterion: the small fixture set maps with explainable results; every deliberately ambiguous case is surfaced rather than silently mislinked. This is a directional proof, not a statistical accuracy claim.

### Phase 1 — useful read-only application

- Migrate/reuse the existing SQLite scan catalogue.
- Incremental background scanning and offline browse.
- Artist Inbox and manual artist mapping.
- TIDAL catalogue cache and release timeline.
- Missing/partial/alternate classification with filters.
- Persistent acquisition checklist and URL/CSV/JSON export.

Exit criterion: the user can scan the real library, resolve artists, review credible gaps, and produce a reliable Tidaler-ready list without changing audio files.

### Phase 2 — integrated fulfilment

- Stable Tidaler adapter with configurable quality and destination.
- Queue progress, cancellation, retry, error detail, and restart recovery.
- Post-download rescan and expected-versus-arrived verification.
- Deduplication/edition controls informed by real usage.

Exit criterion: approved releases can complete end-to-end without duplicate queueing, UI blocking, or lost state.

### Phase 3 — safe library maintenance

- Extract existing organizer logic into services.
- Rich previews for tag/date/folder changes.
- Backups and recoverable quarantine for destructive cases.
- Operation history and verification rescan.

Exit criterion: every proposed filesystem/tag mutation is reviewable and test-covered; interrupted runs leave a recoverable, intelligible state.

### Later enhancements

- Acoustic fingerprints for untagged or incorrectly tagged files.
- Watch-folder or scheduled scans.
- Notifications for newly discovered releases.
- Multiple TIDAL regions/accounts and multiple local libraries.
- Rekordbox/Serato/Traktor metadata interoperability, subject to separate requirements.

## 12. Acceptance criteria for the first useful release

- A second scan of an unchanged library does not reread audio metadata.
- A disconnected library remains browsable from its last saved snapshot.
- No artist is auto-linked from name equality alone when multiple plausible TIDAL artists exist.
- Every automatic mapping displays evidence and can be overridden.
- Catalogue pagination and rate-limit recovery are covered by focused mocked tests plus one small live smoke check.
- Missing results distinguish whole missing releases, partial releases, and alternate editions.
- The queue survives application restarts and prevents accidental duplicate submissions.
- Approved items export as stable TIDAL URLs/IDs usable by Tidaler.
- No download, tag write, move, overwrite, or deletion occurs without an explicit reviewed action.
- Secrets and tokens do not appear in the repository, database, exports, or logs.

## 13. Decisions to validate during the proof phase

These do not block initial architecture, but real-library examples should settle them:

1. Whether “complete artist collection” means primary-artist albums and singles only, or also appearances/compilations.
2. Whether deluxe/reissue bonus tracks count as missing when the standard release is owned.
3. Whether the target is release completeness or unique-track completeness.
4. Preferred behavior for explicit/clean, stereo/Atmos, and remastered variants.
5. Whether direct Tidaler integration is desired after export is proven reliable.
6. Whether the application is strictly personal or intended for distribution; this materially affects authentication and compliance design.

## 14. Existing assets we can reuse

- `music-organizer-tui/library_db.py`: persistent SQLite catalogue, settings, scan snapshots, stale state, and artwork exclusion.
- `music-organizer-tui/music_organizer.py`: tag repair, date normalization, stray-track matching, folder plans, backups, and dry-run behavior.
- `music-organizer-tui/library_view.py`: offline library browsing and tag inspection concepts.
- Tidaler: URL-driven downloads, queue behavior, quality selection, metadata, artwork/lyrics, progress/cancellation, and availability checks.
- python-tidal: useful reference and current dependency for Tidaler, but it is explicitly an unofficial API client and should sit behind an adapter.

## 15. Recommended immediate next step

Build Phase 0 against a read-only sample or a copy of the library. Keep it to 8–12 carefully selected releases rather than a broad catalogue exercise. Do not provide credentials in chat or commit them to either repository. Configure them locally through an ignored environment file or, preferably, the OS credential store. The proof should produce a compact report containing local artist/release evidence, chosen TIDAL IDs, alternatives, confidence, and classified catalogue gaps. That report will let us tune matching and edition policy before investing in the final UI.

To control implementation and model usage, use a risk-based test budget: run fast unit tests only for matching, database migration, queue persistence, and destructive file plans on normal changes; add one mocked API integration test per endpoint family; use a single small live TIDAL smoke test when catalogue behavior changes; and reserve full-library scans, full regression, packaging, and cross-platform checks for release candidates. Do not repeatedly ask the model to inspect large logs or the complete library—save concise machine-generated failure summaries and supply only the failing cases.
