# Tibrary 🎵

**Tibrary** is a local-first desktop application for macOS designed to manage, verify, and complete your local music collection for DJs with local music collections. It links your local files to an \*online streaming catalogue\* (👀), identifies missing releases or incomplete albums, and allows you to safely perform ID3 tag, artwork, and folder operations so that locally downloaded files will match online catalogue releases.

🚨 Primarily built by **Gemini 3.8** and **GPT-6 Astra**, only small fixes and amendments have been done by me

![Overview of Tibrary](./support/imgs/overview.jpg?raw=true)

## What It Does

Tibrary aims to: **scan once, match as many files as possible to the online catalogue automatically, review clearly, and change/add only with explicit approval.** Catalogue matching runs safely in the background; local files are only altered when you say so.

* **Smart Incremental Scanning:** Scans local directories for audio files and supports multiple drives/libraries

* **Deterministic Catalogue Linking:** Matches your local releases to remote catalogue entities by analysing individual track and whole release tags, album/EP/single structures, ISRCs, durations, and mix versions (deluxe, remix etc.) together, never guessing based purely on track title and artist and allowing manual release/artist linking

* **Missing Music Detection:** Automatically maps artist discographies to find gaps between existing releases, flag incomplete albums, and discover new releases

![Missing releases from online catalogue](./support/imgs/missing_releases.jpg?raw=true)

* **Safe Library Maintenance:** Preview date normalizations, track zero-padding (`01` vs `1`), Camelot `INITIALKEY` conversions, and tag-based folder reorganization before writing any changes to disk. Tracks can have local ID3 tag and folder structure fixes applied without linking to an online release, whilst linked releases can have higher resolution artwork and tag enrichment applied

* **MQA Audit:** Includes a read-only 36-bit stereo-XOR protocol check to detect MQA audio streams in FLAC files, allowing you to manually review them for deletion or replacement

* **Isolated Download Queue:** Send missing tracks to a persistent acquisition checklist. Approved items can be exported as URLs or handed off to an isolated background download runtime.

## The Workflow

1. **Update Library:** Scan your local folders to reconcile new, modified, or deleted files.

2. **Prepare & Correct:** Standardize tags, clean up dates, convert musical keys, and preview file organization.

![Enrich local files with tags from linked online releases](./support/imgs/add_tags.jpg?raw=true)

3. **Link Catalogue:** Associate local tracks with online IDs. *Linking only updates the database, not your audio files.*

![Missing releases from online catalogue](./support/imgs/link_releases.jpg?raw=true)

4. **Find Missing Releases:** Compare your local library against online discographies. Spot missing tracks, alternate editions, or entirely missing albums.

5. **Queue & Download:** Approve specific tracks or albums, then export the list or route them directly to the download pipeline.

## Installation & Setup

**Requirements:**

* macOS

* Python ≥ 3.12 (for the main app)

1. **Clone the repository:**

   ```
   git clone https://github.com/elxrrr/tibrary.git
   cd tibrary
   
   ```

2. **Set up the virtual environment & install:**

   ```
   python3 -m venv .venv
   .venv/bin/python -m pip install -e ./app
   
   ```

3. **Launch the application:**

   ```
   ./Start.command
   
   ```

**Optional Tools:**

* **Download Runtime:** Run `./app/tools/Setup\ downloads.command` (requires Python 3.13) to configure the isolated download bridge (`app/resources/tidaler/.venv`).

![Connection setup and status](./support/imgs/connections.jpg?raw=true)

## Credits

* [**Mutagen**](https://mutagen.readthedocs.io/?utm_source=gemini)

* [**Tidaler**](https://github.com/?utm_source=gemini)

* [**python-tidal**](https://github.com/tamland/python-tidal?utm_source=gemini)

* [**AudioAuditor**](https://github.com/Angel2mp3/AudioAuditor?utm_source=gemini)

* **MQA Reverse Engineering** by [purpl3F0x](https://github.com/purpl3F0x/MQA_identifier?utm_source=gemini) and [Dniel97](https://github.com/Dniel97/MQA-identifier-python?utm_source=gemini)