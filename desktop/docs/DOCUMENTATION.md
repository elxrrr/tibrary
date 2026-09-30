# Tibrary guide

Version **0.9.4** · [Setup and screenshots](../../README.md)

## Everyday workflows

| Area | What to do |
| --- | --- |
| Overview | Add a library, inspect indexed/linked counts and browse recent missing releases. |
| Prepare library | Correct local tags, preview folder changes and review redundant local releases. |
| Link catalogue | Match album artists and recordings to online releases; inspect or choose ambiguous placements. |
| Complete library | Find missing releases, select whole releases or individual tracks, and manage downloads. |
| Update library | Audit MQA files, fill missing metadata, improve artwork and review online replacements. |
| Settings → General | Manage the subscriber connection, libraries, appearance, downloads and matching preferences. |
| Settings → Activity | Follow local, online and download jobs; expand a job for individual checks and outcomes. |

### Local changes

Select one correction or organisation operation, then review the affected files before applying it. Previews use the shared local index and are reused until their inputs change. Tag corrections and folder moves are separate actions. Folder previews show current and proposed paths, including whitespace changes.

Track/disc matching treats `01` and `1` equally. Number corrections use padded tags and flag impossible totals such as `04/1`. Consistent local or cached online evidence can supply replacement totals; ambiguous cases require review.

Duplicate removal uses the system Trash and preserves the chosen keeper. MQA signals are audit evidence; replacement is a separate action. Review proposed changes before applying them.

Files changed through the app update its index and dependent views. After editing or moving files in another application, use **Check local changes** from Overview. It reads tags only for added or changed files and records removals. Keep the database: it holds links, cached metadata and saved decisions.

Reviewed moves keep their links and ignore choices; equivalent number formatting does not invalidate a recording link. Scans exclude temporary working audio files. If a file/tag update cannot be indexed, the original path and tags are restored.

### Linking and recommendations

Artist matching starts from local **Album Artist** tags. Compilation placeholders such as “Various artists” are excluded from the artist inbox. Confirmed artists are skipped during unresolved matching; select them explicitly to recheck or unlink.

Recording matches use ISRC, title, duration, release structure and track positions. Linking writes associations to the database, not file tags. Multiple compatible editions can supply metadata, but conflicting totals, mixes or recording evidence require review.

Missing releases defaults to **My album artists**, using album-level artist IDs. Artist scope and recommendation confidence have separate filters, so you can combine **My album artists** with **Recommended**, or inspect other appearances and low-confidence results. Recommendation references come from present local files and their verified online links. Contributor IDs and roles, independent recordings, labels and copyright holders strengthen a match; common distributors, technical credits and genres provide context. Conflicting artists, compilations and unofficial releases cannot become Recommended.

A release is marked superseded only when cached evidence shows a newer available release contains all its recordings. Exclusive mixes remain protected; superseded items remain inspectable.

### Cache and refresh controls

| Action | When to use it |
| --- | --- |
| Check local changes | Update the shared index after editing files outside Tibrary; reuse unchanged tags. |
| Update missing releases | Fill missing evidence for your downloaded, linked releases first, then check artist release lists and complete new, changed or incomplete candidates. |
| More update options → Check release lists only | Check for new releases without collecting additional track details. |
| More update options → Fill missing release artists | Fill album artist evidence for older cached releases. |
| Check availability | Check selected releases, or the visible page; reuse recent market checks. |
| More update options → Check saved release availability | Check all saved missing releases for your linked album artists using cached checks where possible. |
| More update options → Recheck saved availability online | Request fresh checks, including previously unavailable releases; bypass cached availability. |
| More update options → Recalculate saved results | Recalculate ownership and recommendations locally, without API requests. |
| Reread all tags in General | Force a complete tag read if an external editor preserved file size and modification time. |
| Get missing metadata in a release’s metadata view | Complete that release’s missing metadata, reusing checked track details and credits. |
| Refresh track details and credits | Request a fresh check for a particular release. |
| Recheck availability | Request a fresh market-availability check. |

All online workflows use one subscriber connection. Refreshes keep up to three artists in flight, sharing the same adaptive metadata request limit; audio transfers have separate limits. Release lists are checked for new music, then compared with the saved catalogue. Complete details for unchanged releases are reused; new or changed releases and missing data are fetched. This includes audio track lists, artist/contributor credits, genres, recording IDs, release IDs and available BPM/key data. Metadata found during tagging, linking or downloading is shared with the other tools. Activity reports which artist and release is being checked and how much data was fetched or reused. Use **Refresh track details and credits** to request fresh details for a particular release.

Requests reuse pooled connections and market-scoped caches, with shared rate-limit backoff. A release index updates only the relevant cached artist pages; existing databases are indexed automatically without losing links or cached details. Provider errors are not treated as proof that a release is unavailable.

The first recommendation-data fill can take hours for a large library. **Check release lists only** provides a lighter discovery check; full updates fill missing recommendation evidence. Recommendation updates keep up to three complete release-detail checks in flight. Local tag/artwork preparation reads chosen links in one batch and reuses each release within the job. A bounded live check of three releases took 1.07 seconds serially and 0.83 seconds with three workers, without rate-limit errors; large refreshes depend on the amount of uncached data and service response times. More audio download connections do not raise the metadata request limit.

Availability is shared across artist, metadata and release checks for the selected market. Direct release checks are reused for seven days when available, one day when unavailable, and one hour when inconclusive. Artist-list flags are stored separately: they cannot restore a release that a direct check found unavailable. Older cached positives receive a direct check when you use **Check availability**; completed checks are reused. Confirmed unavailable releases leave the default Missing releases view but remain inspectable through **Unavailable**. Timeouts and rate-limit errors (HTTP 429) never mark a release unavailable. Use **Recheck saved availability online** to bypass these checks without resetting the database or rereading track details.

Metadata views show the actual saved release fields, performers and contributor credits. They distinguish unchecked fields, incomplete checks and successful checks where the service supplied no value. A provider/distributor is displayed separately from a record label. Genre names use the catalogue’s `genreName` field. Optional label, genre, provider and replacement checks are cached for 30 days after a complete result; incomplete checks have a shorter retry window and share endpoint cooldowns. These checks do not redownload complete track credits. Raw returned catalogue and track data are retained for future interpretation.

If upstream tags change without changing a release's summary, use its manual detail refresh. Missing fields are not negative matching evidence, and the app does not guess a label or genre when the service supplies none.

Navigating between pages does not start or cancel scans. Jobs finish in the background and publish their results to every relevant view. Catalogue refreshes publish saved artist results while the job is still running, retaining table sorting and expanded tracks. Cancellation retains completed checks. Artwork dimensions and MQA audits are cached per file; unchanged files are reused. Reviewed tag-only moves retain valid inspections, while artwork or relevant MQA tag changes invalidate the corresponding result. Local duplicate checks reuse the saved analysis until the indexed file manifest changes.

BPM, key, genres, credits, UPC and replacement IDs are retained when supplied. Missing fields are not invented. Missing-tag previews preserve existing tags. No developer credentials or database reset are needed.

### Downloads and activity

Choose the destination, template and audio options in General. Approve releases or individual tracks in the queue before starting a download. Videos are excluded. Download progress is separate from local work. Stalled transfers can be cancelled; failed transfers remove their partial files. Downloads publish only after the release has been staged and destination collisions checked. File moves and tag updates keep an original until the database update succeeds. Replacements retain valid existing BPM/key tags when the matching recording has no new values.

MQA replacements use the selected library's existing file paths, including multiple copies of a recording. Previous copies stay recoverable until the replacement audio and queue status are indexed. Recording, mix and duration conflicts stop the replacement for review.

**Export** saves a plain-text list of media URLs, one per line, for an external downloader. Whole approved releases produce album URLs; individually approved tracks produce track URLs. Unapproved and empty selections are omitted. Tidaler accepts this list with `tidaler dl --list acquisition-queue.txt`.

In Downloaded releases, checked releases or tracks limit the export without changing the saved queue. With no checked items, the export uses the saved completed-download selections.

Activity shows item counts, percentage and a smoothed ETA where measurable; discovery remains indeterminate until the total is known. Each channel shows one expandable group per job. Parallel download releases and tracks stay inside their download job; status updates retain the existing rows. Expanded running jobs append saved details automatically, and older history loads as you scroll. Activity is saved by default: job summaries return after reopening, and expanding them loads their saved details. **Save activity logs** must remain enabled to retain history; clearing a panel removes its saved entries.

Closing during downloads asks whether to keep downloading or stop and quit. Completed downloads remain. Interrupted catalogue refreshes retain their checkpoints; explicitly cancelled jobs stay cancelled. File mutations are not automatically replayed.

## Development

The running app uses React/TypeScript for its interface and Rust for scanning, matching, metadata, database access and downloads. Python is only used by the disposable test fixtures.

| Location | Responsibility |
| --- | --- |
| `desktop/src/` | Views, tables, dialogs, navigation and activity. |
| `desktop/src-tauri/src/main.rs`, `actions.rs`, `workflows.rs` | Commands, jobs and state propagation. |
| `db.rs`, `scanner.rs`, `tag_writer.rs` | SQLite state, local indexing and Lofty tag access. |
| `tidal.rs`, `subscriber_metadata.rs`, `network.rs` | Subscriber catalogue, metadata caching and request scheduling. |
| `linking.rs`, `release_matching.rs`, `recommendations.rs` | Recording/release matching and recommendation evidence. |
| `downloads.rs`, `stream_download.rs` | Queue execution and audio transfers. |
| `maintenance.rs`, `organisation.rs`, `duplicates.rs`, `mqa.rs` | File operations, layout, duplicates and MQA auditing. |
| `desktop/tests/` | Disposable file/database fixtures and WebKit workflow tests. |

The macOS database is `~/Library/Application Support/Tibrary/library.sqlite3`. Schema changes must preserve existing data. Never point file-operation tests at a personal library.

### Checks

From the repository root:

```sh
npm --prefix desktop ci
npm --prefix desktop run build
npm --prefix desktop test
cargo test --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary
cargo build --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary
(cd desktop && npx playwright install webkit)
npm --prefix desktop run test:e2e
```

WebKit tests use the debug Rust executable with temporary databases and files. Authenticated live download tests are ignored by default and must be run deliberately with disposable destinations. Passing automated tests does not verify every live catalogue item or native macOS interaction.

Optional focused benchmarks: `metadata_preparation_benchmark -- --ignored --nocapture` uses synthetic database records; `live_metadata_concurrency -- --ignored --nocapture` makes seven read-only credited-release requests through the saved subscriber connection. Add either filter after the Rust test command above. Stop live benchmarking if the service starts throttling.

To refresh the documentation screenshots:

```sh
TIBRARY_SCREENSHOTS=1 npm --prefix desktop run test:e2e -- --grep 'all workflow routes render'
```

These capture the web interface with sample data, not the native macOS window frame.

### Build and release

```sh
npm --prefix desktop run tauri build -- --bundles app
codesign --verify --deep --strict desktop/src-tauri/target/release/bundle/macos/Tibrary.app
```

`Start.command` opens the built app. `desktop/tools/Demo.command` uses a separate demo database.

The editable app icon is `desktop/icon.svg`; platform icon files are in `desktop/src-tauri/icons/`. Regenerate them with the Tauri icon command when the artwork changes.

Before distributing an updated app build, increment the patch version (for example, `0.9.2` → `0.9.3`) in `desktop/package.json`, `desktop/package-lock.json`, `desktop/src-tauri/Cargo.toml`, `desktop/src-tauri/Cargo.lock` and `desktop/src-tauri/tauri.conf.json`. Keep these versions identical; the sidebar reads the package version automatically. Run the checks, refresh screenshots, commit and tag the tested revision. GitHub releases publish source only; do not attach the locally built app or installers. The local build is ad-hoc signed, not notarised.
