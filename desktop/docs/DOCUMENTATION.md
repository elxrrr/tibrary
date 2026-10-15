# Tibrary guide

Version **0.9.36** · [Setup and screenshots](../../README.md) · [Release audit](RELEASE_AUDIT.md)

## Everyday use

| Area | Purpose |
| --- | --- |
| Overview | Indexed and linked counts, your libraries and recent missing releases. |
| Browse music | Explore the online catalogue and choose releases or tracks for the queue. |
| Prepare library | Correct tags, organise folders, review duplicates and flag AI-labelled files. |
| Link catalogue | Match album artists and recordings; review ambiguous release placements. |
| Complete library | Browse missing music, approve releases or tracks, and manage downloads. |
| Update library | Audit MQA, fill missing tags, improve artwork and review online replacements. |
| Settings → General | Account, libraries, appearance, download folders, audio and matching options. |
| Settings → Activity | Search saved activity and cancel individual local, online or download jobs. |

Choose the library at the bottom of the sidebar. Category headings open their first workflow. Overview is the dashboard; its recent-release list scrolls independently.

### Browse music

Open **Browse music** in the sidebar. Explore the new-release feed, search Releases / Artists / Tracks, or paste a TIDAL artist, album or track link. Artist pages separate albums from EPs and singles. Back and Forward return to your previous results.

Open a release to inspect its audio tracks. Choose **Queue full release**, or check individual tracks and choose **Queue selected tracks**. Then open Download queue to review and start the download. Adding more tracks preserves existing approvals; browsing does not add artists to Missing releases or change local links.

No local library is required. Cached pages remain available offline. New searches and uncached release checks need a working account; stale release details must be refreshed before queueing. Page refreshes are independent of library jobs. Artwork loads only as it comes into view and uses the shared disk cache. AI-labelled and known-unavailable items are excluded from discovery and cannot be queued.

### Tables and selection

Missing releases, Download queue and Downloaded releases use the same **album artist → release → track** hierarchy. Expand in place to browse artwork and cached tracks. Artist checkboxes select their displayed releases; release checkboxes select their tracks. Partial selection appears on the parent. Sorting and filters preserve whole groups, expansion and individual selections. Missing releases puts the artist with the newest matching release first.

Small covers load only for visible release rows, are cached on disk and are reused across all three pages. Unavailable artwork does not block browsing. Queue and Downloaded show download status; Missing releases also shows recommendation confidence and coverage. Search includes cached track names and ISRCs.

Local file tables group files beneath releases. Link releases also groups by album artist. Link artists and Favourite artists expand only to local releases. Local duplicates and Online replacements group the release to keep/acquire above the redundant releases, so the proposed outcome remains clear.

Use column headers to sort and their filter buttons to choose checked values. Numeric counts, positions and evidence have no filter menus. Values within a column are combined; filters across columns apply together, before pagination. **Reset column filters** clears them. **Shift+F10** opens a focused header's menu; **Escape** closes it.

Workflow actions sit below the table. The arrow beside the main action contains related operations or scan scopes. Choosing a correction does not apply it: review affected files and their proposed changes first.

## Files and tags

Startup checks for files added, changed or removed while the app was closed. Unchanged tags and audits are reused. Changes made inside Tibrary update the shared index and relevant views as they complete. Navigation neither rereads tags nor cancels work. Make external edits while Tibrary is closed.

- **Correct tags:** preview one correction, inspect the old/new values, then apply. Matching treats `01` and `1` equally. Text tags and filenames use padded numbers. MP4/M4A stores numeric fields as integers, so it is not repeatedly offered an unpersistable padding change.
- **Invalid totals:** values such as `04/1` are flagged. Consistent siblings and cached online placements can supply the correct total; conflicting evidence requires review.
- **Organise files:** inspect current and proposed paths, including whitespace changes. Missing required tags and invalid templates appear as evidence rather than silently disappearing. These rows cannot be applied until corrected.
- **Local duplicates:** review the keeper, affected files and recording/quality evidence. Removal uses Trash. Different mixes, featured performers or lower-quality replacements must not be treated as interchangeable.
- **MQA audit:** choose All releases or Unscanned releases only from Scan's arrow, then scan the checked files. Find online matches and Queue replacements are separate steps. Replacement happens only after approval in the download queue.
- **Missing tags and artwork:** use verified links and shared cached metadata. Existing valid tags are preserved. API fields that are absent are not invented.

Reviewed moves preserve links and ignore choices. File mutations retain an original until indexing succeeds; failed publication restores the original. Tag and move tests verify the encoded audio remains unchanged. Replacements verify the recording and preserve valid local BPM/key values when the replacement provides none.

### Disconnected drives

A cached library remains browsable while its drive is unplugged. Online checks and downloads to a connected folder still work. Scans, tag changes, moves, removal and replacements into the disconnected drive are unavailable. Open reviews respond to drive availability. An offline library is not treated as an empty library; its cached tracks and links remain.

Keep the application database. It contains links, metadata, decisions and history; no database reset is required for an upgrade.

### AI-labelled music

Explicit online AI labels exclude music from suggestions and replacement recommendations. Flagged queue entries remain visible but cannot be approved, exported for download or redownloaded. Download preflight checks retrieved release and track labels.

**Prepare library → AI review** identifies local files using verified links or embedded online IDs. A track label does not label unrelated siblings. Files are never automatically removed. An absent label means unknown, not verified human-made.

## Linking and missing releases

Linking updates database associations. File-tag corrections are reviewed separately. Existing matches are retained when unrelated tags or locations change. Strong, consistent sibling links can resolve another track into the **same release**, provided recording and placement evidence agree. A recording match on a different edition is not proof of the local release's structure. Audio track totals exclude videos.

Link artists uses album artists. Favourite artists shows the catalogue-link state separately from whether the artist is favourited or present locally. Local aliases with the same confirmed online identity can share one row.

Missing releases starts with **My album artists → Recommended and potential**. Overview uses that same scope, independent of temporary table filters. Use View options for artist scope and release range; the Recommendation and Coverage column filters expose low-confidence, superseded or unavailable releases.

Recommendations compare candidates with local recordings and their verified online metadata. Exact recordings, contributor IDs and creative roles, specific labels and copyright holders provide supporting evidence. Genres and generic distributors are weaker context. An artist-page association alone cannot override substantial contradictory evidence. Missing or incomplete credits are neutral. No artist-specific exceptions are used.

A newer release supersedes an older recommendation only when complete cached audio lists contain all its recordings without a known fidelity downgrade. Unavailable, ignored or low-confidence editions do not displace valid recommendations; exclusive mixes remain protected.

### What each online action fetches

| Action | Work performed |
| --- | --- |
| Update missing releases | Fill missing metadata for linked local references, check artist release lists, then complete new, changed or incomplete candidates. |
| Check release lists only | Discover releases without fetching every track's extended details. |
| Fill missing release artists | Complete album-artist evidence for older cached releases. |
| Check availability | Check the selected releases, or visible page, reusing recent market checks. |
| Check saved release availability | Check saved candidates for linked album artists. |
| Recheck saved availability online | Bypass saved availability checks, including previous negative results. |
| Recalculate saved results | Recompute ownership and recommendations without network requests. |
| Get missing metadata | Complete one release's missing details using the shared cache. |
| Refresh track details and credits | Explicitly request fresh details for a release. |

One subscriber account supplies catalogue data and downloads. The market comes from that account and is remembered for offline browsing. No developer credentials are required.

Metadata requests reuse pooled connections, a shared cache, bounded concurrency and adaptive backoff. Audio transfers have separate connection limits. Artist refreshes compare summaries with saved versions; complete unchanged track lists and credits are reused. New, changed or incomplete releases fill their gaps. Successful checks that returned no BPM/key are cached too; omitted fields can still need a lookup. Raw online responses and available genres, UPC, ISRC, credits, dates, audio modes and replacement IDs are retained.

Availability checks are cached for seven days when available, one day when unavailable and one hour when inconclusive. Artist-list flags cannot revive a release withdrawn by a direct check. Rate limits, timeouts and server failures never prove unavailability. Optional label/genre/provider/replacement checks have separate retry windows; they do not redownload complete credits.

A first comprehensive fill can still take hours. More audio download connections do not increase the metadata quota. If upstream tags change without a summary change, use the release's explicit detail refresh. Completed results are saved during work; cancellation keeps them and a saved catalogue refresh can resume.

## Downloads and activity

Set the destination, folder template and audio options in General. Approve whole releases or individual tracks, review the download table, then start. Videos are excluded. Existing output is reused only after checking its audio metadata and recording identity; a conflicting or invalid file produces a reviewable error.

Downloaded releases offers release/track redownload actions without duplicate queue records. Export saves plain text with one album or track URL per line for an external downloader. Checked items limit the export; otherwise saved approvals/completed selections are used. Unapproved and AI-blocked items are excluded. Tidaler accepts the list with `tidaler dl --list acquisition-queue.txt`.

Activity is one searchable log with Action type filters and saved history. Artist refreshes update one row per artist with progress and final outcome. Concurrent audio transfers have separate progress rows. Local, online and download workers have independent cancellation controls, with percentages and smoothed ETA when measurable.

You can browse, filter and approve the queue while independent jobs run. Scans and online reads can run alongside downloads; file-changing jobs and downloads exclude each other. Closing during active work asks whether to continue or stop safely. Completed files remain; unfinished work stays queued. File mutations are never automatically replayed after restart.

## Development and verification

The application uses React/TypeScript and Rust. Python is only used for disposable test fixtures. Runtime data lives at `~/Library/Application Support/Tibrary/library.sqlite3`; migrations must preserve it.

| Location | Responsibility |
| --- | --- |
| `desktop/src/` | Pages, tables, dialogs, selection and activity. |
| `desktop/src-tauri/src/main.rs`, `actions.rs` | RPC commands, workers and publication. |
| `db.rs`, `scanner.rs`, `workflows.rs`, `tag_writer.rs` | Shared index, previews, file metadata and tags. |
| `BrowseView.tsx`, `browse.rs` | Interactive catalogue pages, bounded page caching and queue handoff. |
| `tidal.rs`, `subscriber_metadata.rs`, `network.rs` | Subscriber API, metadata cache and pacing. |
| `linking.rs`, `release_matching.rs`, `recommendations.rs` | Matching and recommendation evidence. |
| `downloads.rs`, `stream_download.rs`, `file_publication.rs` | Transfers and atomic output publication. |
| `maintenance.rs`, `organisation.rs`, `duplicates.rs`, `mqa.rs` | Local changes and audits. |
| `desktop/tests/` | WebKit workflows and disposable audio/database fixtures. |

```sh
npm --prefix desktop ci
(cd desktop && npx playwright install webkit)
sh desktop/tools/check.sh
```

The suite covers Rust services, frontend state and browser workflows with temporary audio and mock HTTP services. FFmpeg is needed for remux checks; HTTP fixtures require localhost access. No personal library is modified.

The bounded live check uses the saved subscriber session, downloads one track into a temporary directory and verifies its audio, tags, queue and disconnected-library behaviour:

```sh
sh desktop/tools/check.sh --live

# Browse, queue one selected track, download and verify temporary audio
env -u TIBRARY_TEST_MODE cargo test --locked --manifest-path desktop/src-tauri/Cargo.toml \
  live_browser_search_to_download_roundtrip -- --ignored --nocapture
```

Live checks refuse credentials close to expiry rather than refreshing the account. Other ignored tests and benchmarks are opt-in; do not run them indiscriminately. See the [release audit](RELEASE_AUDIT.md) for what was actually tested and the remaining manual checks.

```sh
TIBRARY_SCREENSHOTS=1 npm --prefix desktop run test:e2e -- --grep 'all workflow routes render'
npm --prefix desktop run tauri build -- --bundles app
codesign --verify --deep --strict desktop/src-tauri/target/release/bundle/macos/Tibrary.app
```

Screenshots use sample data and exclude the native window frame. `Start.command` opens the built app; `desktop/tools/Demo.command` uses a separate demo database. The editable icon is `desktop/icon.svg`.

Increment the patch version in the frontend package/lockfile, Rust manifest/lockfile and Tauri configuration together. Verify, build, update screenshots and commit the tested source. GitHub releases contain source only; local builds are ad-hoc signed, not notarised.
