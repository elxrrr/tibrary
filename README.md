<div align="center">

# Tibrary 🎵

**A modern, local-first music library manager and catalogue companion app.**

[![Rust](https://img.shields.io/badge/Rust-1.80+-orange?style=flat&logo=rust)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-v2-24C8D8?style=flat&logo=tauri)](https://v2.tauri.app/)
[![React](https://img.shields.io/badge/React-19-61DAFB?style=flat&logo=react)](https://react.dev/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

<br/>

<img src="./desktop/src-tauri/icons/128x128.png" alt="Tibrary App Icon" width="128" height="128" />

</div>

---

## 🌟 Overview

**Tibrary** matches your local music files against official online releases to give your collection rich, comprehensive tags and help you discover new music.

By pairing your local albums and tracks with their streaming counterparts, Tibrary can fill available metadata—including release dates, artwork, disc numbers, Camelot keys and BPM. It also caches track credits, with contributor names and roles, to support release matching and recommendations. Availability depends on what the provider returns; missing credits never become invented metadata.

**Scan your library, review proposed changes, and approve edits before they touch your files.** Duplicate removal uses the system Trash. Catalogue linking itself changes only the app database.

---

## ✨ Features

- ⚡ **Fast Library Scanning:** Quickly scans your local music folders (FLAC, ALAC, MP3, WAV, AIFF) and checks for new or modified files.
- 🎯 **Accurate Catalogue Matching:** Matches your tracks and albums to official online releases using song titles, track lengths, album structure, and ISRCs to fetch comprehensive tags without guessing.
- 🔍 **Track New & Missing Releases:** Compares your local collection against full artist discographies to spot missing albums, bonus tracks, or newly released music.
- 🔬 **MQA Audio Audit:** Scans lossless FLAC files to identify authentic MQA streams so you can easily review, keep, or replace them.
- 🗃️ **Duplicate Finder:** Detects duplicate tracks and alternative releases across your folders, comparing audio formats and quality so you can keep the best copy.
- 🏷️ **Safe Tag & Folder Organization:** Standardizes track numbers, adds Camelot keys and BPM, and organizes folders into clean structures (`Artist/Album (Year)/Track - Title`). Changes are previewed first, and tags are updated without altering your audio quality.
- 📋 **Acquisition Queue:** Keep a checklist of missing tracks or releases you want to collect, with options to export lists or manage approved items.
- ⬇️ **Reviewed Downloads:** Download approved audio with bounded parallel transfers, per-track progress, and recoverable redownloads. Local preparation remains available during downloads.
- 📊 **Clear Activity:** Follow local work, catalogue checks and downloads in separate live panels, with independent search and history controls.
- 🧭 **Flexible Workspace:** Use back and forward navigation beside the window controls. Hide the sidebar to give the current page the full window width; navigation stays available.
- 🧩 **Credit Evidence:** Shared composers, songwriters, producers and other credited contributors can support recommendations. Only current verified local links or local tags provide reference evidence; shared names never override recording or release-structure conflicts. Credits, including empty results, are cached for reuse.

---

## 🖼️ Visual Tour

Screenshots below use a disposable sample library.

<div align="center">
  <img src="./desktop/docs/imgs/overview.png" alt="Overview with persistent navigation and a scrollable latest missing releases list" width="90%" />
</div>

### 1. Prepare & Clean Library
Inspect your library, scan for modified files, audit for MQA streams, and resolve duplicate tracks.
<div align="center">
  <img src="./desktop/docs/imgs/prepare_library.png" alt="Tibrary Prepare Library Overview" width="90%" />
</div>

<br/>

### 2. Link Artists
Match album artists from local tags to online artist profiles across collaborations and alias variations.
<div align="center">
  <img src="./desktop/docs/imgs/link_artists.png" alt="Link Artists Interface" width="90%" />
</div>

<br/>

### 3. Match Releases
Pair local tracks with official releases to review track mappings, lengths, and album editions.
<div align="center">
  <img src="./desktop/docs/imgs/link_releases.png" alt="Link Releases and Track Mappings" width="90%" />
</div>

<br/>

### 4. Spot Missing Music & Track New Releases
Audit artist discographies to find uncollected releases or new singles and add them to your queue.
<div align="center">
  <img src="./desktop/docs/imgs/missing_releases.png" alt="Missing Releases Detection" width="90%" />
</div>

<br/>

### 5. Review Local Duplicates
Compare audio formats, bit depths, and sample rates to clean up duplicate files safely.
<div align="center">
  <img src="./desktop/docs/imgs/local_duplicates.png" alt="Review Local Duplicates" width="90%" />
</div>

<br/>

### 6. MQA Audio Audit
Inspect FLAC audio frames to detect authentic MQA encoding.
<div align="center">
  <img src="./desktop/docs/imgs/mqa_audit.png" alt="MQA Detection and Audit" width="90%" />
</div>

---

## 🚀 Quick Start

```sh
brew install ffmpeg

# Install frontend dependencies
npm --prefix desktop ci

# Launch the app in development
npm --prefix desktop run tauri dev
```

On macOS, you can also launch the prebuilt application directly using `./Start.command`.

To build a local macOS application with Node.js and the Rust toolchain installed:

```sh
npm --prefix desktop run tauri build -- --bundles app
open desktop/src-tauri/target/release/bundle/macos/Tibrary.app
```

---

## 🙏 Credits & Attributions

- [**Lofty**](https://github.com/Serial-ATA/lofty-rs) — Audio tagging and metadata library in Rust.
- [**Tauri**](https://v2.tauri.app/) — Desktop application framework.
- [**Turso / libsql**](https://github.com/tursodatabase/libsql) — Embedded SQLite database engine.
- [**AudioAuditor**](https://github.com/Angel2mp3/AudioAuditor) by Angel2mp3 — MQA signal detection reference.
- **MQA Reverse Engineering Credits** — Pioneered by [purpl3F0x](https://github.com/purpl3F0x/MQA_identifier) and [Dniel97](https://github.com/Dniel97/MQA-identifier-python).

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).

### Safe number repairs and quitting

Correct tags → Track & disc numbers flags impossible totals such as `04/1`. It proposes padded totals from consistent sibling tags or agreeing cached releases with matching recordings and positions. Incomplete or conflicting evidence stays for review. Applying a number repair refreshes the affected links from cache without an online scan.

Closing the window or choosing Quit while downloading asks whether to keep downloading or stop and quit. The latter waits for download cancellation before exiting. Completed downloads are retained.

### Catalogue evidence and library previews

- Missing tags uses provider genres (track first, release fallback) and UPC when supplied, preserving existing tags. Genre and replacement relationships share the catalogue cache; missing data is upgraded as releases are inspected. Empty provider responses are valid, and optional lookup failures retain cached data.
- Recording links still require release/position evidence. UPC agreement ranks otherwise valid editions; provider replacement IDs add candidates without overwriting saved identities. Genres provide recommendation context, never identity proof.
- Missing releases marks an older release **Superseded** only when a newer, available release contains every exact recording, including its mix and duration. These entries remain accessible with the Superseded or All recommendations filter. Unloaded track lists and exclusive mixes are not assumed redundant.
- Ownership uses distinct, current recording links and checks multi-disc completeness. Deleting the database is unnecessary: refresh the catalogue and rerun previews while preserving your saved decisions.
- Organise files shows exact current/proposed paths and component-level reasons. Years use four digits; slash-form track numbers parse correctly; Unicode/case-only differences do not propose moves. Multi-disc filenames use `02.01 - Title`.
- MQA signal badges are red; no-signal results are green. Unaffected rows show no replacement action.

### Cached release checks and refresh progress

- **Recheck cached releases** recalculates ownership and recommendations in a background task using saved data. It does not contact the service, rewrite tags or move files. Activity reports its completion.
- Missing-release totals count missing, incomplete and queued releases across the cached catalogue. The table also shows how many releases match its current filters; the Overview card opens the corresponding unfiltered missing list.
- Catalogue refreshes show the current artist, market and scope in Activity. Progress is saved after each artist. Interrupted refreshes resume when the app next opens; explicit cancellation stays cancelled. **Resume refresh** continues a stopped or failed refresh using its saved scope. Local file changes and downloads are not automatically replayed.

Compilation placeholders such as “Various artists” stay out of the artist inbox and bulk artist matching; their individual file and release links remain intact. Refresh progress uses completed-artist counts, displays small nonzero progress as `<1%`, and shows `Working` until the first artist completes.

Linking recognises both legacy and current file signatures, avoiding unnecessary rechecks of unchanged linked tracks. Activity retains each checked track’s link/review/unmatched outcome. The header task indicator opens Activity and includes the current work details; MQA audit is under Update library.
