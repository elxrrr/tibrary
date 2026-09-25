# Tibrary — Complete Technical Documentation & Reference Manual

> **Version:** 0.9.0-beta.3 (build 3)  
> **Target Platforms:** macOS 13+ (Apple Silicon & Intel), Linux, Windows 10/11  
> **Core Stack:** Tauri v2 · Rust 1.80+ · Turso / libsql · React 19 · Lofty  

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

## Verification status — 25 September 2026

This remains a beta, not a certified public release. The Rust migration audit restored previously unhandled desktop actions and removed silent-success fallbacks. Linking and extended review write database associations only; filesystem edits require a current, explicit preview. File changes invalidate reviewed writes. Jobs run independently of navigation, with lightweight progress polling and revision-based table caches.

Connections now consist of application credentials for the official catalogue API and **one shared subscriber sign-in** for favourites, extended metadata and audio downloads. These are distinct authorization types; duplicate subscriber connection controls were removed. Account tokens are saved in macOS Keychain. No Python service is required.

Verification uses disposable files/databases, mocked catalogue data, and WebKit against the actual Rust RPC backend. Routine Rust tests do not consume live API quota; credential-store/live catalogue tests are explicitly ignored unless requested. A read-only snapshot of the production library was used to exercise all table routes, without modifying library files. On that 19,243-track snapshot, cold link-table construction took about 7.5 seconds; cached filtering took 16–17 ms. Cold-start query optimization remains useful future work.

Live official catalogue authentication, artist search and audio-only release pagination were checked. Cassie release `140303440` returns 12 audio tracks, excluding its video. Successful subscriber OAuth completion, favourites retrieval and a real authenticated audio download still require an available subscriber session and are **not verified by the automated checks**. Native decoding, tagging, staging, queue selection and missing-account failure paths are tested separately. Non-macOS packaging and notarized public distribution are not validated by this audit.

## 1. Architecture Overview

Tibrary is designed with a local-first, dual-process desktop architecture pairing a high-performance **Rust native core** with a reactive **React 19** user interface:

```
┌────────────────────────────────────────────────────────────────────────┐
│                        React 19 Desktop UI                             │
│   • TypeScript · Tailwind CSS · Lucide Icons · Vite Bundler            │
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
| `tidal.rs` | Direct native Tidal API client with OAuth2 token persistence, search, artist discography pagination, and rate limiting. |
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
