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

By pairing your local albums and tracks with their official streaming counterparts, Tibrary accurately fills in missing metadata—including full album credits, release years, cover artwork, disc numbers, and DJ tags like Camelot keys and BPM. At the same time, it follows your favourite artists across their entire discography so you can easily track new singles, EPs, and albums you don't have yet.

Everything is local and non-destructive: **scan your library, review proposed changes clearly, and apply edits or sync updates only when you explicitly approve.**

---

## ✨ Features

- ⚡ **Fast Library Scanning:** Quickly scans your local music folders (FLAC, ALAC, MP3, WAV, AIFF) and checks for new or modified files.
- 🎯 **Accurate Catalogue Matching:** Matches your tracks and albums to official online releases using song titles, track lengths, album structure, and ISRCs to fetch comprehensive tags without guessing.
- 🔍 **Track New & Missing Releases:** Compares your local collection against full artist discographies to spot missing albums, bonus tracks, or newly released music.
- 🔬 **MQA Audio Audit:** Scans lossless FLAC files to identify authentic MQA streams so you can easily review, keep, or replace them.
- 🗃️ **Duplicate Finder:** Detects duplicate tracks and alternative releases across your folders, comparing audio formats and quality so you can keep the best copy.
- 🏷️ **Safe Tag & Folder Organization:** Standardizes track numbers, adds Camelot keys and BPM, and organizes folders into clean structures (`Artist/Album (Year)/Track - Title`). Changes are previewed first, and tags are updated without altering your audio quality.
- 📋 **Acquisition Queue:** Keep a checklist of missing tracks or releases you want to collect, with options to export lists or manage approved items.

---

## 🖼️ Visual Tour

### 1. Prepare & Clean Library
Inspect your library, scan for modified files, audit for MQA streams, and resolve duplicate tracks.
<div align="center">
  <img src="./desktop/docs/imgs/prepare_library.png" alt="Tibrary Prepare Library Overview" width="90%" />
</div>

<br/>

### 2. Link Artists
Match local artist folders to verified online artist profiles across collaborations and alias variations.
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
# Install frontend dependencies
npm --prefix desktop ci

# Launch the app in development
npm --prefix desktop run tauri dev
```

On macOS, you can also launch the prebuilt application directly using `./Start.command`.

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
### Download container support

Install FFmpeg with `brew install ffmpeg` before downloading on macOS. Some lossless streams arrive in MP4 containers; Tibrary copies their encoded audio into FLAC without re-encoding. No Python runtime is required.
