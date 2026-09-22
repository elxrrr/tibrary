# Tibrary

A desktop app for organising tagged music, linking local releases to an online catalogue, and reviewing missing music for download.

## Launch

Double-click [Start.command](../../Start.command) in the application folder.

Supporting commands live in `app/tools/`:

- [Demo.command](../tools/Demo.command) opens a fictional, separate library.
- [Setup downloads.command](<../tools/Setup downloads.command>) installs the optional download runtime using Python 3.13.

For a fresh application installation, see [Development and setup](reference/DEVELOPMENT.md).

## Suggested workflow

1. **Overview → Update library** reconciles new, changed and deleted files with the index. Unchanged tag inspections are reused. Use **Recheck all tags** for editors that preserve timestamps.
2. **Prepare Library** handles local work. **Correct Tags** standardises dates, track/disc numbers and musical keys, or removes embedded lyrics. **Organise Files** previews moves based on saved tags. Choose and review one operation at a time.
3. The **Prepare Library** dashboard summarises local preparation, **MQA Audit** and **Local Consolidation**. Consolidation verifies retained recordings and asks you to review moving redundant files to Trash. Exclusive mixes and different performer versions must be retained.
4. **Link Catalogue → Link Releases** saves recording/release associations in the database. Linking does not edit music files. Whole-release structure, recording identifiers, mix versions and durations help distinguish editions. Multiple proven equivalent online IDs can remain associated with one local release. Ambiguous matches remain reviewable.
5. **Fix Library** provides separate **Add Missing Tags**, **Fix Artwork** and **Online Replacements** workflows. Review proposals before applying file changes. Online replacements are downloaded before any reviewed consolidation of originals.
6. **Complete Library → Missing Releases** lets you compare newer releases, releases between your newest two, all missing releases, or incomplete albums. Check releases or expand them to select tracks, then choose **Queue selected**. Availability must be confirmed before queueing.
7. **Download Releases → Queue** holds the saved checklist. Approving or clearing an album selects or clears its child tracks. Approve only what you want, then explicitly start downloading or export the list.

Use **Settings → Activity** for progress, retries and errors. Context menus provide metadata inspection, local file reveal and actions relevant to the selected records.

## Tags, artwork and paths

- Album Artist drives library grouping. Track Artist remains the performer credit; folder names do not replace these tags.
- `01` and `1` compare equally during linking. Numeric tag standardisation uses at least two digits. Missing or impossible totals can be proposed from agreeing sibling tags or verified cached release placements; file count alone never proves an album is complete. Conflicting totals remain for review.
- Existing BPM, keys and other DJ analysis are preserved by missing-tag enrichment. Supported online fields are filled only when available for a verified recording; ambiguous values are withheld. Conflicting album credits do not silently overwrite Album Artist.
- **Correct Tags** can explicitly convert recognised keys to Camelot `INITIALKEY`. Unknown/conflicting keys remain unchanged.
- Lyrics are not acquired. Removing existing embedded lyrics is a separate optional correction.
- Artwork targets genuine **1280 × 1280** front covers. Larger square covers can be downsized; smaller images are not upscaled. Other embedded pictures are preserved.
- Folder organisation follows saved tags and the template in **Settings → Downloads**. Disc folders/prefixes are used where appropriate; custom templates are retained.
- Portable filenames preserve accents and literal hyphens. Slash/colon separators become spaced hyphens, unsupported characters are removed, and trailing dots/spaces are trimmed. These path rules do not rewrite title or album tags.

Changed sources, symlinks and destination collisions are guarded against. File operations update the index; changes affecting identity invalidate the relevant linking state. External edits/deletes require a library update—there is no filesystem watcher. Existing link/cache data survives ordinary restarts and navigation.

## Connections and downloads

Configure application credentials and account sessions in **Settings → Connections**. Configure audio quality, output location and download options in **Settings → Downloads**. Lossless FLAC is the default. Downloads use the tag-based folder layout, omit lyrics and preserve existing different files.

**Update streaming components…** builds a separate runtime and checks compatibility before activation. **Restore previous components** rolls back a successful update. Failed builds leave the active runtime in place. Component updates do not update the application itself.

## Documentation

- [Development and setup](reference/DEVELOPMENT.md): installation, source layout, storage and safe tests.
- [Linking and UI architecture](reference/LINKING_AND_UI_AUDIT.md): matching rules, scores, identity safeguards and shared state.
- [Release checks](reference/RELEASE_CHECK.md): recorded verification and outstanding acceptance work.
- [Third-party attribution](THIRD_PARTY.md): MQA references and licensing.
- [Original product brief](reference/PRODUCT_BRIEF.md): historical requirements, not current operating instructions.

## Cached online metadata

Release details are shared between discovery, linking, missing-tag and artwork workflows, scoped to the selected market. Structural details use the release-cache age setting (30 days by default). Fresh album/track enrichment and extended BPM/key responses use a one-day cache; rechecking link decisions can reuse saved album evidence for the longer release-cache window. Expired or insufficient records are fetched again; failed requests preserve previously saved data. A changed release item count invalidates its cached track list.

Rechecking unresolved links reuses this evidence instead of forcing every network request again. Successful links stay saved until relevant local identity changes or an explicit recheck. Linking collects BPM/key analysis in the database and stages missing tags; it does not write tags to music files. Provider metadata can be unavailable or omit BPM/key—these cases do not fabricate values or overwrite existing DJ tags.
