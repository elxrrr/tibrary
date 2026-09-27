# Tibrary — Complete Technical Documentation & Reference Manual

> **Version:** 0.9.0-beta.34 (build 34)
> **Target Platforms:** macOS 13+ (Apple Silicon & Intel), Linux, Windows 10/11  
> **Core Stack:** Tauri v2 · Rust stable · Turso SQLite · React 19 · Lofty

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [Developer Setup & Workflow](#2-developer-setup--workflow)
3. [Testing Suite & Verification](#3-testing-suite--verification)
4. [Product & Technical Specification](#4-product--technical-specification)
5. [Database Schema & Persistence](#5-database-schema--persistence)
6. [Linking Architecture & Scoring Model](#6-linking-architecture--scoring-model)
7. [MQA Protocol & Signal Detection](#7-mqa-protocol--signal-detection)
8. [Third-Party Dependencies & Attributions](#8-third-party-dependencies--attributions)
9. [Release Verification & Packaging Checklist](#9-release-verification--packaging-checklist)

---

## Verification status — 26 September 2026

Build 11 makes Activity status describe the current task and clears completion after three seconds. Connection checks show progress and results on their button rather than in the window header. Local duplicate work now uses the local worker lane, with scan progress visible in Activity. Release matching settings separate unofficial, compilation and fuzzy-title review choices. Unofficial and compilation releases remain visible for manual inspection; automatic linking requires the corresponding permission. The local duplicate scan compares every recording with a distinct retained track and verifies FLAC sample rate and bit depth before suggesting removal. Cached duplicate results are reused until the indexed file manifest changes. MQA audit rows load from the index and keep valid per-file audit evidence; new or changed files are marked for recheck. File mutations signal view refreshes and clear stale previews. Catalogue cache entries retain optional original release dates, audio modes, media metadata and contributor credits when supplied; recommendation scoring only uses contributor overlap when it is anchored to local tagged music. Similar-artist, radio and playlist relationships are not fetched during routine scans because they are weak evidence for release ownership and would add requests for every artist.

This remains a beta, not a certified public release. The Rust migration audit restored previously unhandled desktop actions and removed silent-success fallbacks. Linking and extended review write database associations only; filesystem edits require a current, explicit preview. File changes invalidate reviewed writes. Jobs run independently of navigation, with lightweight progress polling and revision-based table caches.

Connections use **one shared subscriber sign-in** for discovery, favourites, extended metadata and audio downloads. Developer authentication and its settings have been removed. Account tokens are saved in macOS Keychain. No Python service is required.

Verification uses disposable files/databases, mocked catalogue data, and WebKit against the actual Rust RPC backend. Routine Rust tests do not consume live API quota; credential-store/live catalogue tests are explicitly ignored unless requested. A read-only snapshot of the production library was used to exercise all table routes, without modifying library files. On that 19,243-track snapshot, cold link-table construction took about 7.5 seconds; cached filtering took 16–17 ms. Cold-start query optimization remains useful future work.

Build 10 gives local, online and download tasks separate running states and in-memory activity buffers. Each Activity panel can be searched, copied and cleared independently. Overview displays the newest 50 cached missing releases in a scrollable card. The missing-release index now prefers album-artist tags across both metadata layouts, treats complete standard/deluxe editions as owned by default, and uses cached UPC, ISRC, label, copyright and verified primary-artist evidence when present. "Recheck cached releases" rebuilds recommendations without spending API quota; it does not reread files on disk. The catalogue and account connection dates are written on first successful connection and do not change on later diagnostics. Only the subscriber catalogue integration is active: MusicBrainz, Discogs and Spotify are not queried, so their release-group identifiers are used only if already present in cached data.

Live subscriber authentication, artist search and audio-only release pagination were checked. Cassie release `140303440` returns 12 audio tracks, excluding its video. An isolated authenticated one-track download and two-track parallel download passed using the saved account; both wrote only to temporary folders. A copied real FLAC passed local preview, tag application, MQA audit, and source-integrity checks. Segmented transfers and recoverable redownload publishing have focused tests. The cached-catalogue copyright-object format that caused the Online replacements and Optimizations deserialization failure now parses correctly. Non-macOS packaging and notarized public distribution are not validated by this audit.

## 1. Architecture Overview

Tibrary is designed with a local-first, dual-process desktop architecture pairing a high-performance **Rust native core** with a reactive **React 19** user interface:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        React 19 Desktop UI                             │
│   • TypeScript · CSS · Lucide Icons · Vite Bundler                     │
│   • Virtualized Data Tables (Multi-Sort, Multi-Select, Cascades)       │
│   • Reactive State Synchronization via Atomic Database Revisions       │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ Tauri v2 IPC / CLI JSON-RPC
┌───────────────────────────────────┴────────────────────────────────────┐
│                    Native Rust Application Engine                      │
│  ┌───────────────────────────────┬──────────────────────────────────┐  │
│  │ Storage & Data Services       │ Audio & Format Services          │  │
│  │ • Turso / libsql Embedded DB  │ • Lofty Metadata Engine          │  │
│  │ • Schema Migrations           │ • FLAC Padding-Aware Tag Writer  │  │
│  │ • Atomic Revision Tracking    │ • 36-Bit Stereo-XOR MQA Detector │  │
│  │ • Fast Bulk Indexing          │ • Audio Frame SHA-256 Hashing    │  │
│  ├───────────────────────────────┼──────────────────────────────────┤  │
│  │ Networking & Catalogues       │ Matching & Resolution            │  │
│  │ • Native reqwest Tidal Client │ • Multi-stage Candidate Linker   │  │
│  │ • Token Lifecycle Management  │ • Whole-Release Structure Gates  │  │
│  │ • Exponential Backoff & Cache │ • ISRC & Duration Tolerances     │  │
│  ├───────────────────────────────┼──────────────────────────────────┤  │
│  │ Streaming & Downloads         │ Maintenance & Organization       │  │
│  │ • AES-256-CBC Token Decrypt   │ • Safe Move, Rename & Clean      │  │
│  │ • AES-128-CTR Stream Decrypt  │ • Chained Duplicate Containment  │  │
│  │ • MPEG-DASH / BTS Manifests   │ • Atomic Staging & Publishing    │  │
│  │ • PKCE Browser Auth Flow      │ • DJ Camelot Key & BPM Enriched  │  │
│  └───────────────────────────────┴──────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────┘
```

### Core Rust Modules (`desktop/src-tauri/src/`)

| Module | Responsibility |
| --- | --- |
| `actions.rs` | Reviewed background actions, shared release-detail cache, metadata/artwork previews, MQA audit and consolidation review. |
| `main.rs` | Application entry point, window management, system menus, Tauri IPC command routing, and headless `--rpc` server. |
| `db.rs` | Embedded Turso/libsql database engine, schema migrations, table views, and atomic revision counters. |
| `scanner.rs` | Fast, non-blocking local filesystem crawler, path normalization, and audio metadata extraction via Lofty. |
| `tag_writer.rs` | Safe audio metadata editor using Lofty. Preserves FLAC padding, custom DJ tags, and embedded artwork without modifying audio frames. |
| `stream_download.rs` | Native Tidal stream decryptor (AES-256-CBC token decryption, AES-128-CTR stream decryption), MPEG-DASH / BTS manifest parser, PKCE OAuth flow, Vorbis comments tagger, and `.lrc` / `.m3u8` companion file generator. |
| `downloads.rs` | Acquisition queue state management, quality selection, pacing delay coordinator, and atomic publishing pipeline. |
| `tidal.rs` | Native subscriber catalogue client: search, exact ISRC lookup, paginated discographies, batched summaries and shared market-scoped evidence caches. Uses the same subscriber session as downloads. |
| `matching.rs` | Confidence-scored artist and track candidate matching, ISRC resolution, and structural duration gating. |
| `release_matching.rs` | Whole-release track alignment, multi-disc normalization, and candidate ranking. |
| `linking.rs` | High-level library linking pipeline connecting local tracks to confirmed Tidal releases. |
| `enrichment.rs` | Missing DJ tag calculations (Camelot musical keys, BPM normalization). |
| `musical_keys.rs` | Musical key conversions and Open Key / Camelot wheel notation mapping. |
| `mqa.rs` | 36-bit stereo-XOR sync detection algorithm for verifying authentic MQA streams directly in raw PCM frames. |
| `maintenance.rs` | Safe directory relocation, file renaming, and duplicate consolidation. |
| `organisation.rs` | Layout path templating and file system safety checks. |
| `duplicates.rs` | Local duplicate finder and containment analyzer; detects chained absorption patterns (Single → EP → Album) and executes safe bulk deletions. |
| `recommendations.rs` | Contributor network traversal and artist discography gap discovery. |
| `account.rs` | Tidal user session management, keychain integration, and favourite sync. |
| `workflows.rs` | Orchestration of background library workflows (scanning, matching, retagging, auditing). |

---

## 2. Developer Setup & Workflow

### Prerequisites
- **macOS** 13+ (Apple Silicon or Intel), **Linux**, or **Windows 10/11**.
- **Rust Stable** (1.80 or later): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Node.js** (v20 or v22 LTS) and **npm**: `brew install node`
- **FFmpeg**: `brew install ffmpeg`. Used only to copy lossless audio from MP4 containers into FLAC, without re-encoding. Tibrary finds standard Homebrew installations when launched from Finder.
- **No Python runtime required**: The application engine, decryption, manifest parsing and tagging run in Rust.

### Quick Start
```sh
# 1. Install frontend dependencies
npm --prefix desktop ci

# 2. Run in development mode (hot-reload UI + native Rust backend)
npm --prefix desktop run tauri dev

# 3. Build release bundle (.app and .dmg)
npm --prefix desktop run tauri build
```

### Headless RPC & Demo Modes

#### Headless RPC Mode
For automation, continuous integration, and headless scripting:
```sh
./desktop/src-tauri/target/debug/tibrary --rpc --db /path/to/test.db
```
Clients exchange newline-delimited JSON (NDJSON) requests (e.g. `{"id": 1, "method": "state.get", "params": {}}`) over standard I/O (`stdin` / `stdout`).

#### Isolated Demo Mode
To test the interface with synthetic library data without accessing real files:
```sh
./desktop/tools/Demo.command
# Or manually:
export TIBRARY_DEMO=1
./desktop/src-tauri/target/release/bundle/macos/Tibrary.app/Contents/MacOS/tibrary
```

---

## 3. Testing Suite & Verification

Tibrary maintains a comprehensive multi-tier testing pipeline:

```sh
# 1. Rust unit and integration tests (41 tests across DB, Lofty, MQA, Matching, Stream Decryption, Tagging, Tidal API)
cargo test --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary

# 2. Strict Rust linter check (0 warnings allowed)
cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary -- -D warnings

# 3. React frontend unit tests (Vitest)
npm --prefix desktop test

# 4. End-to-end headless integration tests (Playwright)
npm --prefix desktop run test:e2e
```

The Playwright E2E suite uses `desktop/tests/seed_desktop.py` to populate a temporary Turso/libsql database with synthetic FLAC fixtures, testing table interactions, approval cascades, and settings resets without touching any live media.

---

## 4. Product & Technical Specification

### Guiding Principles
- **Scan once, match deterministically, review clearly, and change only with approval.**
- Catalogue discovery and gap analysis are strictly read-only.
- Downloads, tag edits, moves, and deletions remain explicit actions with previews and auditable results.

### Key Capabilities

1. **Smart Incremental Scanning:** Traverses audio roots without following symlinks. Files are fingerprinted via `(path, size, mtime_nanoseconds)`. Unchanged files skip tag parsing; modified or new files are read via Lofty.
2. **Deterministic Catalogue Linking:** Matches local releases to Tidal catalogue entities using multi-evidence scoring: exact ISRCs, track durations (±3s window), multi-disc alignments, edition variants (Deluxe, Remaster, Explicit), and whole-release structure.
3. **Missing Music Discovery:** Cross-references confirmed local artist holdings against complete Tidal discographies (Albums, EPs, Singles) to identify missing releases, historical catalogue gaps, and incomplete albums.
4. **MQA Signal Audit:** Runs a bit-accurate 36-bit stereo-XOR sync pattern detector directly on raw PCM audio frames to distinguish authentic MQA streams from standard lossless audio and misleading file tags.
5. **Duplicate & Replacement Inspection:** Identifies duplicate tracks and chained multi-release containment patterns (e.g. Single ⊆ EP ⊆ Album) across folders. Displays duplicate clusters under their master keeper album and executes bulk deletions safely to macOS Trash in a single operation.
6. **Non-Destructive Tag & Folder Maintenance:** Previews tag normalizations (leading zeros, Camelot `INITIALKEY` conversions, BPM standardization) and directory reorganizations (`Artist/Album (Year)/Track - Title`).
7. **Acquisition Queue & Fulfilment:** Persistent, reviewable acquisition queue. Export approved items as URLs/JSON/CSV, or dispatch them directly to Tibrary's built-in native streaming download engine.

### Download & Tagging Architecture

- **100% Native Rust Engine:** Downloads are executed directly by Tibrary's asynchronous Tokio streaming engine (`stream_download.rs` and `downloads.rs`) without Python. MP4-wrapped FLAC uses an external FFmpeg stream-copy step.
- **Audio Qualities:** Supports `LOSSLESS` (16-bit / 44.1 kHz FLAC), `HI_RES_LOSSLESS` (up to 24-bit / 192 kHz FLAC), `HIGH` (320 kbps AAC), and `LOW` (96 kbps AAC).
- **Stream Decryption:** Decrypts 32-byte master security tokens with AES-256-CBC and audio stream bytes with AES-128-CTR in real time.
- **Streaming Manifests:** Seamlessly parses BTS JSON manifests and MPEG-DASH MPD XML manifests with segment templates and timeline offsets.
- **Album Artist Isolation:** The release's album artist is strictly mapped to `ALBUMARTIST`. Individual track guest artists or remixers remain in `ARTIST`, preventing track performer pollution and ensuring multi-artist albums stay grouped under the album artist folder.
- **Vorbis Comment & ID3 Tagging:** Writes lossless Vorbis comments for FLAC files, embedding front-cover JPEG artwork, ReplayGain tags (`REPLAYGAIN_TRACK_GAIN`, `REPLAYGAIN_TRACK_PEAK`, `REPLAYGAIN_ALBUM_GAIN`, `REPLAYGAIN_ALBUM_PEAK`), lyrics, and Tidal identifiers.
- **Staging & Safe Publishing:** Files are downloaded into an isolated temporary staging directory, tagged with verified metadata, and published into the target folder layout (`{album_artist}/{album}/{track_number} {title}`) using atomic moves. Existing user files are checked for SHA-256 idempotency and collision prevention.
- **Comprehensive Settings:** Supports skipping existing files, companion `cover.jpg` saving, embedded lyrics, `.lrc` companion lyrics files, `_playlist.m3u8` playlist generation, pacing delays, and ReplayGain volume tags.

---

## 5. Database Schema & Persistence

All state is stored locally in an embedded **Turso / libsql** SQLite database (`library.db`):

### Core Tables
- `settings`: Key-value configuration for library paths, download destination, API credentials, and thresholds.
- `libraries`: Configured local library roots and scan timestamps.
- `files`: Indexed audio files, metadata tags, fingerprints, and scan status.
- `track_links`: Durable associations between local file paths and Tidal track/release IDs, including candidate evidence and manual link flags.
- `tidal_cache`: Persisted API responses (artist details, release listings, tracklists) with market scoping and expiration timestamps.
- `queue_items`: Pending, active, completed, or failed acquisition tasks.
- `ignored_releases`: User-specified ignore rules for specific releases or artists.

### Reactive Revision Tracking
The database maintains an in-memory `Arc<AtomicU64>` revision counter. Every write operation (ignoring releases, unlinking tracks, queueing downloads, updating settings) increments this counter. The React frontend monitors the revision key; when it increments, active table views reload current data instantly without resetting scroll position or expansion state.

---

## 6. Linking Architecture & Scoring Model

### Multi-Tier Resolution Pipeline
1. **Tier 1 (Embedded Identifiers):** Tidal Track ID, Album ID, UPC barcode, or exact ISRC code.
2. **Tier 2 (Normalized Artist Alignment):** Normalized artist name lookup, discography retrieval, and release title overlap scoring.
3. **Tier 3 (Whole-Release Structural Gating):**
   - **Duration Gate:** Rejects candidates whose durations differ by more than **3.0 seconds**.
   - **ISRC Conflict Gate:** Known conflicting ISRCs immediately reject candidates.
   - **Mix/Version Gate:** Acoustic, remix, live, instrumental, radio edit, and extended mix tags must match candidate recording text.
   - **Cardinality Gate:** An incomplete 3-track local EP is never forcibly aligned to a 10-track album edition unless track numbers and titles map to an exact subset.
4. **Tier 4 (Evidence Scoring & Tie-Breaking):**

| Evidence Criterion | Points | Rationale |
| --- | ---: | --- |
| Exact Case-Insensitive Album Title | 40 | Exact title match is strong primary evidence. |
| Normalized Album Title | 25 | Matches with minor punctuation or capitalization variances. |
| Matching Release Year | 20 | Confirms contemporaneous release date. |
| Matching Declared Total Track Count | 25 | Confirms identical edition structure. |
| Matching Declared Disc Count | 15 | Confirms multi-disc parity. |
| Recording Identifier Evidence (ISRC/ID) | 20 | Confirms recording lineage. |

- **Maximum Theoretical Score:** 120 points.
- **Auto-Acceptance Threshold:** `S ≥ 60` with a margin of at least **15 points** over any competing candidate edition.
- **Manual Overrides:** Any manual link or unlink action is marked `manual = 1` and is permanently preserved across subsequent automatic scans.

---

## 7. MQA Protocol & Signal Detection

The native 36-bit stereo-XOR MQA signal analyzer in `desktop/src-tauri/src/mqa.rs` verifies authentic MQA encoding directly from raw uncompressed PCM samples:

- **Protocol:** Scans stereo 16-bit or 24-bit PCM audio frames for the proprietary 36-bit synchronization pattern across stereo channels using XOR correlation.
- **Header Parsing:** Decodes the MQA metadata stream to determine the original master sample rate (e.g. 44.1 kHz, 88.2 kHz, 96 kHz, 192 kHz, 352.8 kHz) and unfold capabilities.
- **Reliability:** Distinguishes genuine MQA bitstreams from misleading file tags, standard lossless FLAC files, and upsampled audio.
- **Attribution:** Based on public reverse-engineering implementations by Angel2mp3 ([AudioAuditor](https://github.com/Angel2mp3/AudioAuditor)), Stavros Avramidis ([purpl3F0x/MQA_identifier](https://github.com/purpl3F0x/MQA_identifier)), and [Dniel97/MQA-identifier-python](https://github.com/Dniel97/MQA-identifier-python).

---

## 8. Third-Party Dependencies & Attributions

### Native Rust Backend (`desktop/src-tauri/Cargo.lock`)
- **`tauri` (v2)** (MIT OR Apache-2.0) — Native desktop application framework and IPC runtime.
- **`libsql`** (Turso) (MIT) — Embedded, high-performance SQLite database engine.
- **`lofty`** (MIT OR Apache-2.0) — Fast, lossless audio tagger; preserves FLAC padding blocks, ID3 frames, and embedded artwork.
- **`reqwest`** (MIT OR Apache-2.0) — Asynchronous HTTP client for Tidal Web API endpoints.
- **`tokio`** (MIT) — Asynchronous I/O runtime.
- **`sha2`** (MIT OR Apache-2.0) — Cryptographic SHA-256 audio frame integrity verification.
- **`aes`**, **`cbc`**, & **`ctr`** (MIT OR Apache-2.0) — Pure-Rust AES-256-CBC token decryption and AES-128-CTR audio stream keystream decryption.
- **`quick-xml`** (MIT) — Fast, zero-allocation MPEG-DASH MPD XML manifest parsing.
- **`claxon`** (Apache-2.0) — Pure-Rust FLAC audio frame decoding and playback verification.

### Desktop Frontend (`desktop/package.json`)
- **`react` & `react-dom`** (v19) (MIT) — Declarative user interface components.
- **`lucide-react`** (ISC) — Modern UI iconography.
- **`vite`** (MIT) & **`typescript`** (Apache-2.0) — Build pipeline and static type checking.
- **`vitest`** (MIT) & **`@playwright/test`** (Apache-2.0) — Unit and end-to-end testing frameworks.

### Attributions & Design References
- **[Tidaler](https://github.com/maya-doshi/tidaler)** (AGPL-3.0) by Maya Doshi — Streaming token exchange, quality modes, and decryption protocol reference for Tibrary's native Rust streaming downloader.
- **[AudioAuditor](https://github.com/Angel2mp3/AudioAuditor)** by Angel2mp3 — MQA signal detection reference.
- **MQA Reverse Engineering** — Pioneered by [purpl3F0x](https://github.com/purpl3F0x/MQA_identifier) and [Dniel97](https://github.com/Dniel97/MQA-identifier-python).

---

## 9. Release Verification & Packaging Checklist

### Automated Verification Pipeline
```sh
# 1. Compilation & type safety
cargo check --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary
npm --prefix desktop run build

# 2. Strict linter checks (0 warnings)
cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary -- -D warnings

# 3. Unit and integration tests
cargo test --manifest-path desktop/src-tauri/Cargo.toml --bin tibrary
npm --prefix desktop test

# 4. Playwright E2E workflows
npm --prefix desktop run test:e2e

# 5. Build native application bundle
npm --prefix desktop run tauri build
```

### Pre-Release Functional Checklist
| Domain | Verification Item | Status |
| --- | --- | :---: |
| **Database** | Embedded Turso/libsql database initializes cleanly from empty or existing SQLite file. | PASS |
| **Database** | Atomic revision counters trigger instant UI table reloads on record mutations. | PASS |
| **Scanner** | Filesystem scan traverses nested directory structures, ignoring symlinks. | PASS |
| **Scanner** | Fingerprinting correctly detects new, modified, or deleted files. | PASS |
| **Audio Engine** | Lofty reads FLAC, MP3, M4A, ALAC, WAV, and AIFF metadata accurately. | PASS |
| **Tag Writer** | FLAC tag updates reuse existing metadata padding without rewriting audio frames. | PASS |
| **Tag Writer** | Pre- and post-write SHA-256 frame hashes guarantee zero audio alteration. | PASS |
| **MQA Audit** | Native 36-bit stereo-XOR sync detector identifies genuine MQA PCM streams. | PASS |
| **Linking** | Exact ISRC, track duration, title, and release structure gates prevent false matches. | PASS |
| **Downloads** | `ALBUMARTIST` is protected and isolated from track guest artists; files layout correctly. | PASS |
| **Queue** | Acquisition queue persists across application restarts and prevents duplicate jobs. | PASS |
| **Security** | Tidal OAuth tokens are stored in the OS credential store and never logged. | PASS |

### macOS Distribution & Notarization
For distributing binaries outside local development environments:
1. Configure `Developer ID Application` signing identity in `desktop/src-tauri/tauri.conf.json`.
2. Build the signed bundle: `npm --prefix desktop run tauri build`.
3. Submit `.dmg` for notarization:
   ```sh
   xcrun notarytool submit desktop/src-tauri/target/release/bundle/dmg/Tibrary_*.dmg \
     --keychain-profile "AC_PASSWORD" --wait
   xcrun stapler staple desktop/src-tauri/target/release/bundle/dmg/Tibrary_*.dmg
   ```

### Authenticated verification — 25 September 2026

A real account check, catalogue detail request and selected-track download completed against an isolated test database. The resulting file decoded without errors as 16-bit/44.1 kHz FLAC, with 1280×1280 embedded artwork, companion cover.jpg, zero-padded numbers and no lyrics. Country parameters were restored on subscriber API requests; official pagination retains the /v2 prefix and country. Progress updates replace one running Activity entry while errors and completion remain visible.

Three disposable copies of existing FLAC files passed scan, read-only preview, reviewed number-tag application, MQA audit and duplicate analysis. Original NVME files were not modified. This verifies those exercised paths, not every possible catalogue edition or download format.

Validation for this patch: 49 Rust tests passed (2 opt-in live tests skipped), 2 selection tests passed, and 12 WebKit workflows passed. A separate authenticated catalogue test traversed 34 releases across pages. Organisation moved two copied files and refused the duplicate destination for the third, preserving all files. The app remains labelled beta; these results are not a claim that every provider catalogue edge case has been verified.


### Table and metadata regression fixes — build 8

- Every launch starts on Overview. Saved job failures remain in Activity and no longer trigger a fresh startup error.
- Overview counts missing releases using the same filtered release data as Missing releases. Processed rows are reused until the database revision or date changes.
- Download review loads every approved queue item, including items outside the current page/filter, and shows exact selected tracks. Release-detail dialogs accept the native catalogue fields.
- Metadata is presented as labelled tables, with local/proposed values and distinct release/track source IDs. Raw JSON is confined to an optional export-data view.
- Automatic local previews retain file size/modification stamps, support reviewed application, and keep an all-files view available. Page-size changes reload tables; double-clicking a disclosure arrow does not open a dialog.
- Local duplicates retain their cached groups and display release dates before replacement destinations. Adding dates to existing cached groups uses indexed tags rather than recomputing duplicate matches.

Validation used disposable fixture libraries and an isolated copy of the production database. The snapshot contained 90,714 missing release rows; subsequent sorts took 0.15–0.25 seconds. Initial state loading still took about 13 seconds in the debug build, so cold-start performance remains a known limitation. No original NVME files were changed.


### Navigation and credit evidence — build 12

- Persistent back/forward controls and a sidebar toggle stay beside the native window controls. Hiding the sidebar expands the content pane. Startup still opens Overview.
- Overview library and latest-missing lists scroll independently; the latter displays at most five 64-pixel rows.
- Release detail requests use `include=items,items.credits`. Older cached releases can fetch only missing credits through batched `/tracks?include=credits` requests. Contributor relationship pagination is followed, while names, roles, category IDs and artist IDs remain attached to their source track. Successful empty responses are cached; incomplete optional requests wait at least a day before retrying. A manual release refresh can recheck them.
- Credits support ranking structurally valid link candidates and appear in their evidence. They cannot override conflicting ISRCs, mixes, durations or positions. Recommendation evidence can come from current verified linked recordings even when the local tags have no credits; unverified recommendations never become evidence for other recommendations.
- The read-only live check for release 234657671 returned six tracks and no credit entries. Mock compound responses verify populated credits and pagination detection. Availability is provider-dependent. README screenshots use disposable sample data, not the live music library.

Validation: 72 Rust tests passed (5 opt-in checks skipped), two focused WebKit workflows passed (including all-page rendering and sidebar/history interactions), and the opt-in live credits request passed. The macOS app bundle was rebuilt. Tests used disposable files and databases; original NVME audio files were unchanged.


### Subscriber-only migration — build 31

All active online workflows now use one subscriber account. Developer credential entry, credential mutation routes, client-credential authentication and the alternate metadata refresh option have been removed. Existing developer secrets are not read or required; existing catalogue and link data are retained.

- Artist/track searches use subscriber search endpoints. Exact ISRC lookup uses `/v1/tracks?isrc=…`, verifies the returned ISRC, and narrows candidate album titles before loading whole releases. Text-searching an ISRC is not equivalent and returned unrelated results in the live probe.
- Discography discovery combines paginated albums and EPs/singles. Compilation appearances are requested only when compilation recommendations are enabled. Main Missing releases and Overview lists default to verified local album artists; broader and unknown results remain accessible through filters.
- Release summaries use batches of up to 20 IDs and share the raw summary cache with artwork/downloads. Fresh summaries populate artist-credit and availability evidence. Audio counts exclude videos. Cached older optional null lists are accepted without treating malformed objects as valid lists.
- Track details/role credits, DJ metadata and artwork continue using shared subscriber caches. The newer catalogue endpoint accepts the subscriber token too: optional genres, replacement IDs, label and original date are fetched in batches of 20 and cached per market for 30 days. Optional access failures pause these lookups for one hour without blocking core workflows or repeatedly failing for every release. Empty/missing fields retain previously cached values. No database reset is needed.
- All catalogue lookups pass the selected country and cache it separately. A failed summary batch can fall back to individual IDs, omitting only confirmed 404 responses; authentication, throttling and persistent server failures retain saved data. HTTP pacing, retry limits and host-wide throttling remain in effect. API timing varies: the bounded comparison found subscriber discovery faster for ATRIP, slower for Canopy, and unsuitable for direct timing comparison for Cassie when compilation appearances were included. It is not universally faster.

Validation: 99 offline Rust tests, 3 frontend unit tests and 27 WebKit workflows passed. Read-only authenticated tests exercised artist/track search, discography pagination, batched summaries, exact ISRC lookup, GB availability, credited audio tracks, DJ metadata, cache reuse and 649 favourites. A selected Cassie track downloaded into a temporary folder, completed the queue, scanned, and linked against cached whole-release data. The 13-item Cassie release is correctly treated as 12 audio tracks. Original NVME files were not written or moved. This checks the exercised workflows, not every provider endpoint or every regional catalogue edge case.

Endpoint reference: [Minim subscriber API implementation](https://minim.readthedocs.io/en/latest/_modules/minim/tidal.html). This is an implementation reference, not an installed dependency.

The selected live FLAC was independently checked with ffprobe: 16-bit/44.1 kHz audio and embedded 1280×1280 JPEG artwork. The subscriber token also returned HTTP 200 from the v2 optional catalogue metadata endpoint; the final workflow test verifies this route and its cache without developer credentials.


## Build 32 — progress, album artist accuracy and QA

### Implementation

- `progress.rs` owns per-job, phase-aware estimates. A monotonic-clock 30-second speed window is smoothed with an 80/20 moving average. Resumed/cached counts establish the baseline instead of inflating speed. Nested release lookups cannot reset overall track-link progress. Percent/count/ETA fields reach both events and status snapshots; finished jobs release estimator state.
- Activity cards and the clickable header share the same progress values. Unknown-size discovery displays “Measuring workload” and ETA estimating; stale results are marked as waiting. A precise ETA cannot be promised before work is measured or while the service is stalled.
- Scanning enumerates audio paths once, then reads only changed tags against an exact total. Cancellation preserves missing-file status; unreadable directory enumeration cannot mark an entire subtree missing. MQA decoding and duplicate comparisons run on blocking workers; duplicate cancellation retains the previous results.
- `network::client` shares connection pools across catalogue, metadata, favourites and artwork workflows. Credited-album prefetch uses up to three concurrent requests through the existing global adaptive limiter; deduplication, per-release gates, cache-first reads and 429 cooldown remain in force.
- Missing-release names prefer the subscriber album summary, then explicit album-level saved metadata/credits, then the release’s own artist field. They never infer the album artist from a track performer or combine names from artist discovery pages. The existing primary-artist market filter remains separate.
- Correction preview joins use a path index instead of repeatedly scanning the file list.

### Reproducible QA

Run `cargo test --manifest-path desktop/src-tauri/Cargo.toml`, `npm --prefix desktop test`, then build the debug backend and run `npm --prefix desktop run test:e2e`.

The WebKit suite exercises every main route and the shared table/selection components: bidirectional sorting, missing-release paging/filtering, parent/child approvals, queue/requeue, context menus, matching review, preview/apply on disposable FLACs, MQA/duplicate scans, navigation during jobs, progress/ETA, cancellation feedback, failed-query recovery, settings, themes, sidebar/history and sign-in redirect validation/cancellation. Rust tests cover actual cache/database and file operations, structural linking, credits, market isolation, throttling/cancellation and parallel segmented downloads.

Explicit live checks use temporary databases/output directories: `live_subscriber_workflows`, `live_parallel_cache`, and `live_subscriber_download_pipeline` (pass `-- --ignored --nocapture`). They exercise search, discography, market availability, favourites, full credits, optional catalogue metadata, cache reuse without authentication, real FLAC download, tag indexing and cached linking. The live download fixture also deliberately changes its temporary tags and filename, then verifies number-total repair and organisation while preserving BPM and audio duration. Never point mutation fixtures at a real music library.

Scope limits: these checks do not prove every possible OS/theme/accessibility combination or remote catalogue response. New-account browser approval, platform packaging outside macOS, and long-duration network outages still need release QA. No new pause mechanism is introduced; cancellation and existing catalogue refresh checkpoints remain supported.

**Build 33 verification:** 104 Rust tests, 5 frontend unit tests and 32 WebKit workflow tests passed. The live subscriber fixture downloads and redownloads a disposable FLAC, verifies the queue has only one entry, applies local number repairs and folder organisation, then reviews and removes a redundant copy through the app's consolidation action. The retained file and database are checked afterwards. NVME originals are never written by these tests. The macOS bundle is built locally and ad-hoc signed.

### General settings and artist review

Settings navigation contains General and Activity. General owns the single Connection card (with expandable diagnostics), Download folder & layout, and a separate Audio and file options card. Legacy internal connections/downloads destinations redirect to General. Template variables use individual chips with explanations.

Artist review filters use current resolved state before inspecting historical evidence. Review match opens the existing candidate dialog; rechecking targets the selected artists and reuses cached evidence. Unlink removes primary and additional artist associations together, preserving recording links, cached catalogue data and music files.

![General settings with one connection card and template-variable chips](imgs/general_settings.png)

### Job activity history

New activity records include job ID, kind and status. An additive `activity_logs.job_context` column keeps existing databases and their logs intact. The live buffer is bounded to 1,000 entries per stream; complete saved details load automatically on expansion and scroll through paginated `logs.job` requests, without separate history buttons. Progress callbacks keep one current progress row while archiving distinct step messages under the job. Start and finish records retain the same ID, so completed jobs remain grouped. Disabling Save activity logs retains the live window only. Pre-migration logs do not have inferred job ownership.

Finder Trash operations run asynchronously with a 30-second timeout. Fallback Trash names use unique IDs to avoid collisions between concurrent replacements; failed Trash operations retain files and return an error.


**Build 34 UI follow-up:** Increased native traffic-light insets and reserved header space. Activity keeps live-row identity and stable progress-summary heights. Saved history loads on expansion/scroll. Table headers use opaque light/dark surfaces, including modal tables and tables refreshing in the background. Targeted WebKit checks cover navigation, automatic history, stable live activity and opaque table headers.
