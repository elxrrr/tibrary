# MQA protocol references

The read-only audit in `app/library_manager/mqa_audit.py` implements the 36-bit stereo-XOR protocol described by:

- [AudioAuditor — MqaDetector.cs](https://github.com/Angel2mp3/AudioAuditor/blob/main/AudioAuditor.Core/Services/MqaDetector.cs), Angel2mp3, Apache License 2.0 (copy in `app/resources/AudioAuditor-LICENSE.txt`). Repeated sync detection and rate field interpretation informed this implementation. The implementation here uses integer PCM, vectorised searching, consistent rate fields, and distinct metadata-only results.
- [MQA-Toolkit](https://github.com/Angel2mp3/MQA-Toolkit), read as a reference; its application code is not bundled.
- The underlying reverse engineering is credited by AudioAuditor to Stavros Avramidis ([purpl3F0x/MQA_identifier](https://github.com/purpl3F0x/MQA_identifier)) and [Dniel97/MQA-identifier-python](https://github.com/Dniel97/MQA-identifier-python).

A three-second scan can establish repeated MQA signal evidence; its absence cannot certify an entire recording as non-MQA. Metadata tags can be inaccurate. Original rate is reported from an embedded field or explicitly labelled tag, never inferred from encoded bit depth.

## Desktop distribution

The Tauri shell and React interface use their upstream licences, with dependency inventories in `desktop/package-lock.json` and `desktop/src-tauri/Cargo.lock`. The Python distribution comes from uv's pinned CPython standalone builds. Installed Python packages retain their `dist-info` licences in the bundle.

The packaged streaming runtime includes the checked-out Tidaler (AGPL-3.0) and python-tidal sources, with their licences, under `Contents/Resources/runtime/sources/`. FFmpeg is provided by the `imageio-ffmpeg` wheel; its executable and licence notices remain in that package. FFmpeg supplies container extraction without requiring a system installation. Review the included upstream licences and corresponding-source requirements before public distribution.

## Native metadata and audit services

`native/tibrary-tags` uses [Lofty](https://github.com/Serial-ATA/lofty-rs) 0.25.3
(MIT OR Apache-2.0), PyO3 (MIT OR Apache-2.0), and RustCrypto SHA-2
(MIT OR Apache-2.0). Exact versions are pinned in its Cargo.lock. The build
includes native dependency licence files and the application-owned Rust source
under `runtime/sources/native`.

The application's metadata operations use Lofty. Mutagen remains an upstream
streaming dependency and an independent test oracle; it is not used by the
application's scanner, maintenance, artwork, audit or download-tagging paths.
The Rust MQA detector retains the protocol and attribution described above.
