<div align="center">
<img src="desktop/src-tauri/icons/128x128.png" alt="Tibrary icon" width="96" />

# Tibrary

**Clean up your files, link your collection, and find missing music.**
</div>

![Tibrary overview](desktop/docs/imgs/overview.png)

## What it does

- **Prepare your library:** preview tag corrections, organise folders and review local duplicates.
- **Link your collection:** match album artists, recordings and whole releases using cached catalogue evidence.
- **Find missing music:** discover new releases and incomplete albums, filter recommendations, and queue selected tracks.
- **Update your files:** fill available tags—including BPM and Camelot keys—improve artwork, and audit MQA files.
- **Follow the work:** one searchable activity log with action filters, saved details and separate cancellation controls for local tasks, online checks and downloads.

## Run on your Mac

Install Xcode Command Line Tools, a current stable [Rust toolchain](https://rustup.rs/), and Node.js LTS. Then, from this repository:

```sh
brew install node ffmpeg
npm --prefix desktop ci
npm --prefix desktop run tauri build -- --bundles app
open desktop/src-tauri/target/release/bundle/macos/Tibrary.app
```

After building, use **Start.command** to reopen the app. Releases contain source code only; binaries are built locally.

For development:

```sh
npm --prefix desktop run tauri dev
```

## Screenshots

Captured from the current UI with a disposable sample library.

| Link releases | Missing releases |
| --- | --- |
| ![Link releases](desktop/docs/imgs/link_releases.png) | ![Missing releases](desktop/docs/imgs/missing_releases.png) |

| General settings | Activity |
| --- | --- |
| ![General settings](desktop/docs/imgs/general_settings.png) | ![Activity](desktop/docs/imgs/activity_log.png) |

See the [guide](desktop/docs/DOCUMENTATION.md) for workflows, cache behaviour and development checks.

## Credits and licence

Built with [Tauri](https://v2.tauri.app/), [React](https://react.dev/), [Lofty](https://github.com/Serial-ATA/lofty-rs) and [Turso](https://github.com/tursodatabase/turso).

MQA detection references: [AudioAuditor](https://github.com/Angel2mp3/AudioAuditor), [purpl3F0x](https://github.com/purpl3F0x/MQA_identifier) and [Dniel97](https://github.com/Dniel97/MQA-identifier-python).

Subscriber API and download references: [Tidaler](https://github.com/maya-doshi/tidaler) and [Minim](https://minim.readthedocs.io/en/latest/_modules/minim/tidal.html).

[MIT licence](LICENSE).
