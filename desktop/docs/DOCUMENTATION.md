# Tibrary guide

Version **0.9.1** · [Setup and screenshots](../../README.md)

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

Select one correction or organisation operation, scan, then review the affected files before applying it. Tag corrections and folder moves are separate actions. Folder previews show current and proposed paths, including whitespace changes.

Track/disc matching treats `01` and `1` equally. Number corrections use padded tags and flag impossible totals such as `04/1`. Consistent local or cached online evidence can supply replacement totals; ambiguous cases require review.

Duplicate removal uses the system Trash and preserves the chosen keeper. MQA signals are audit evidence; replacement is a separate action. Review proposed changes before applying them.

Files changed through the app update its index and dependent views. After editing or moving files in another application, update the library from Overview. Keep the database: it holds links, cached metadata and saved decisions.

### Linking and recommendations

Artist matching starts from local **Album Artist** tags. Compilation placeholders such as “Various artists” are excluded from the artist inbox. Confirmed artists are skipped during unresolved matching; select them explicitly to recheck or unlink.

Recording matches use ISRC, title, duration, release structure and track positions. Linking writes associations to the database, not file tags. Multiple compatible editions can supply metadata, but conflicting totals, mixes or recording evidence require review.

Missing releases defaults to **My album artists**, using album-level artist IDs. Other appearances and unchecked artist credits remain available through the filters. Recommendation badges are evidence-based suggestions, not guarantees. Shared contributors, labels, rights, genres and verified recordings provide context; they do not override recording conflicts.

A release is marked superseded only when cached evidence shows a newer available release contains all its recordings. Exclusive mixes remain protected; superseded items remain inspectable.

### Cache and refresh controls

| Action | When to use it |
| --- | --- |
| Update library on Overview | Local files or tags changed outside Tibrary. |
| Update recommendation data | Collect missing release details, credits and available DJ metadata. |
| Check release artists | Fill album artist evidence for older cached releases. |
| Recheck cached releases | Recalculate ownership and recommendations from saved data, without API requests. |
| Refresh track details and credits | Refresh metadata for a particular release. |
| Recheck availability | Request a fresh market-availability check. |

All online workflows use one subscriber connection. Refreshes keep up to three artists in flight, sharing the same adaptive metadata request limit; audio transfers have separate limits. Recent artist names and track credits are reused for 30 days, while release lists are checked for new music. Requests reuse pooled connections and market-scoped caches, with shared rate-limit backoff. A release index updates only the relevant cached artist pages; existing databases are indexed automatically without losing links or cached details. Provider errors are not treated as proof that a release is unavailable.

BPM, key, genres, credits, UPC and replacement IDs are retained when supplied. Missing fields are not invented. Missing-tag previews preserve existing tags. No developer credentials or database reset are needed.

### Downloads and activity

Choose the destination, template and audio options in General. Approve releases or individual tracks in the queue before starting a download. Videos are excluded. Download progress is separate from local work.

Activity shows item counts, percentage and a smoothed ETA where measurable; discovery remains indeterminate until the total is known. Each job has an expandable history that loads automatically. Save activity logs must be enabled to retain history across restarts.

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

Update the npm, Cargo and Tauri versions together with the sidebar version. Run the checks, refresh screenshots, commit and tag the tested revision. GitHub releases publish source only; do not attach the locally built app or installers. The local build is ad-hoc signed, not notarised.
