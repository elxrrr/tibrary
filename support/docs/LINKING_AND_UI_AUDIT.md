# Linking and desktop interaction audit

## Scope and safety

This is an update to the existing PySide6/SQLite application, not a framework replacement. Linking writes associations to the database. It does not write music tags, rename files, download audio, or retire originals. Tag correction, organisation, download approval and consolidation retain separate actions.

Development uses temporary databases and music fixtures. The isolated test runner blocks external-volume writes and external network connections. No production music, credentials or link caches were cleared. The disposable top-level `pasted-text.txt` and Finder `.DS_Store` files were removed.

## 1. Linking architecture: the actual decision path

### 1.1 Local identity and work scheduling

1. The scanner indexes local file paths, size, modification time and tags. Album Artist drives local grouping; Track Artist remains the performer credit. Folder names locate the release group but do not supply replacement tag text.
2. `release_matching.group_key` combines the containing release folder, normalized album-artist identity and album title. Disc/CD subfolders are collapsed to the release folder. Distinct local editions in distinct directories remain distinct groups.
3. `linking.state_signature` fingerprints indexed local files, durable links, artist mappings and remote cache state. The background scheduler uses this plus pending-path checks, cancellation and completion state to avoid restarting unchanged passes.
4. A file link is reusable when its fingerprint agrees, or its tags changed in a known compatible way. Numeric padding is now compared numerically for track/disc numbers and totals, including slash forms: `1/10` equals `01/10`. This does not equate different track positions or totals.
5. Manual choices survive automatic inconclusive results. The linker version is now 5, so old unresolved placement decisions can be reconsidered. It does not forcibly erase established links.

### Artist identity matching is a separate stage

`matching.BatchMatcher` first checks exact-name favourited artists, then global search when favourites do not explain the local releases. It initially fetches six ranked artist summaries, caches them, and examines further exact-name IDs while local release titles remain unexplained. Unfetched summaries remain visible as unverified candidates.

For artist ID scoring, let `k` be the count of distinct non-generic local release titles found remotely and `f` the fraction of local release titles matched. If `k = 0`, the score is zero. Otherwise:

`artist_score = min(98, round(name_points + 25*f + support))`

`name_points` is 40 for an exact normalized artist name, otherwise 15. `support` is 10 for one release, otherwise `min(33, 20 + 4*(k-2))`. This score is explanatory. Following the earlier requested product policy, enabled automatic artist matching accepts **every checked artist ID with at least one matching local release title**; the saved threshold/margin preferences are not the acceptance gate for this policy. Manual mappings are preserved. Artist-ID acceptance does not itself establish any track/release placement; the stricter recording and structure checks below still apply. This distinction matters when interpreting “matched artist” versus “linked track”.

### 1.2 Candidate discovery and cache

6. `tag_review.check_album_tags` gathers releases from mapped-artist discographies and detail caches, indexed by album title, date, recording ISRC and saved IDs. Existing release IDs and linked siblings supply additional candidates.
7. When cached candidates do not fit, recording lookup can supply further placements. Each detail response is reused within a pass and persisted by market/release ID. Routine linking respects the configured release-link cache lifetime; metadata/artwork checks use their existing freshness policy.
8. Previously, more than eight candidates could terminate processing, even when whole-release evidence could prove a match. That cap and a twelve-release truncation in recording discovery are removed. All discovered candidates are evaluated with cancellation and the existing request pacing. This supports N equivalent placements; it does **not** mean indiscriminately searching the entire provider catalogue.

### 1.3 Recording and release gates

9. Known duration differences greater than three seconds reject a recording. Known conflicting ISRCs reject it. Mix/version distinctions reject incompatible radio, extended, remix, live, instrumental, club, edit or acoustic versions. A matching saved ID does not override a known recording conflict.
10. A matching ISRC or saved recording ID supplies recording evidence. Without these, title and duration evidence must agree, within an appropriate album/artist or established sibling context.
11. Whole-release structure aligns every available local file to the candidate track list. Declared disc/track positions are checked first; a unique recording can identify a shifted position. No remote slot can be claimed twice.
12. Local declared track totals are compared against release-wide or per-disc totals. Known different edition cardinalities block automatic placement. A two-track standalone release therefore does not become a three-track rolling release merely because its two recordings recur there.
13. Files numbered 1, 2 and 5 with a declared total of 10 are an incomplete ten-track release, not a three-track single. Missing positions remain explicit. Impossible local totals smaller than the observed local file count/position are treated as repairable tag defects, not reliable edition evidence.
14. Total discs, unique alignments and structural conflicts are retained in candidate evidence. A single incompatible candidate is now described as a candidate with a structural conflict, not multiple available editions.

### 1.4 Equivalent editions and scoring

15. Equivalent editions require a complete loaded track list with unique disc/track positions, matching normalized release title/date/type/explicitness/count, and a matching ordered recording signature with known durations. That signature includes normalized title, ISRC, duration, performer credits and explicitness. A barcode is an additional guard when complete recording evidence is absent. Alternate barcodes alone do not divide releases whose entire recording signatures agree.
16. Album-level credits are no longer part of the *audio-equivalence* signature. Conflicting album credits are retained as a metadata conflict. Track performer credits **remain** in that signature: a guest-vocal version is not collapsed into an instrumental or differently credited version.
17. A preferred saved/local-product edition can lead the group when available; otherwise the available edition is preferred, then freshness and a stable ID order. Every equivalent album/track pair remains on the chosen option and in durable `equivalent_ids`; there is no one- or two-link ceiling.
18. For structurally compatible non-equivalent placements, the existing score remains:

   `S = title + year + tracks + discs + recording`

   | Evidence | Points |
   | --- | ---: |
   | Exact case-insensitive album title | 40 |
   | Otherwise normalized album title | 25 |
   | Matching release year | 20 |
   | Matching declared total track count | 25 |
   | Matching declared disc count | 15 |
   | Recording identifier evidence | 20 |

   Exact and normalized title points are alternatives. This is an evidence score with a maximum of 120, **not a probability**. The best ambiguous placement needs `S >= 60` and a lead of at least 15 over the next candidate. A unique proven placement or a compatible established same-album sibling placement can link without this tie-break score. Structural gates always run before scoring.
19. A sole recording identity with multiple album-artist grouping choices can save its recording link while leaving the tag choice unresolved. Linking confidence and tag-correction confidence are separate.

### 1.5 Persistence and enrichment

20. `save_result` verifies the file fingerprint before committing the result. It saves the chosen IDs, equivalent pairs, candidate evidence, local-tag baseline, timestamps, manual status and linker version in `track_links`. A stale response cannot silently attach itself to a changed file.
21. A saved result that no longer matches the current file no longer leaves stale `linked_ids`/candidate fields attached to a reused in-memory row.
22. Enrichment consults only the chosen recording placement and its proven equivalents. For a missing field, one nonempty value across supplying sources can fill a gap; conflicting values are withheld. Existing valid values remain protected. Album Artist and Track Artist are excluded from this cross-fill pool.
23. Official catalogue metadata remains primary; the subscriber metadata adapter supplies missing supported values. BPM and musical key are checked independently of whether linking succeeded. Missing DJ metadata is not an unlinked-recording diagnosis. Keys are converted to Camelot for `INITIALKEY`; lyrics are not acquired.
24. Normalization of downloaded and proposed track/disc numeric tags uses at least two digits. Local standardization uses the same validated number policy. Values above 99 are not truncated. Native integer-only tag formats retain their format's numeric representation.

## 2. Canonical artist policy and the named edge cases

### Gouryella / Matt Fax

An established album placement supplies candidate context, not permission to accept unrelated audio. If the recording and whole-release structure agree with the canonical Matt Fax album, legacy local performer text does not strand the track: the recording can link to the parent release. A contrary ISRC, mix, duration or genuine edition total still blocks automatic linking. Album Artist and Track Artist are never forced to the same value.

### Manila Killa / SATICA

A library containing many SATICA tracks and no Manila Killa tracks does **not** prove that SATICA owns a particular album. Local popularity, folder names and how many equivalent provider IDs repeat a credit are not votes for canonical authorship.

Resolution policy:

1. Preserve a user's explicit album-artist decision.
2. Use compatible established album links to reconcile recording placement.
3. Keep complete album credits and individual grouping alternatives visible as tag-review evidence.
4. When audio-equivalent releases disagree on Album Artist, save their recording links but withhold automatic Album Artist changes; expose the conflict in the match dialog.
5. Missing-tag cross-fill never supplies Album Artist/Track Artist from alternate editions.
6. Distinct track performer/mix signatures are not equivalents and require their own recording evidence.

The current provider adapter does not always retain authoritative primary-versus-featured roles. Consequently the application cannot truthfully decide every collaborative-versus-misattributed credit automatically. Where the source data cannot establish that distinction, the resolution is to retain the local credit and request an explicit tag decision. It must not invent “Manila Killa” from the example alone. These rules are tested with synthetic conflicting-credit editions; they are not a claim that every named release in the live catalogue has been manually verified.

## 3. UI/state architecture and implemented changes

### Tables and selection

- `RowsModel` owns the active sort column/order. Replacement data is sorted with the same settings. Parent/child blocks stay together; stable release/track IDs identify expansion and selection.
- Missing Releases snapshots selection keys and both scroll offsets during rebuilding. Expanded release IDs live outside the row list. Sorting by Coverage no longer silently reverts when tracks are expanded.
- Missing Releases has a Select column with parent, partial and child states. Draft selection lives in a release-ID map (`None` = whole release, a set = selected track IDs, empty = clear). Select visible, clear, and Queue Selected act without starting downloads. Releases whose availability is not confirmed are skipped with a count; selecting them does not override that guard.
- Queue approval is an atomic database update of both `approved` and `selected_tracks`. Clearing a parent clears its children; approving it selects the whole release. Child toggles produce a subset or clear the parent when none remain.
- Queue sorting sorts whole parent/child blocks before rendering. Native tables now enable bidirectional header sorting; source-row identities are stored separately so sorted artist, release and track actions target the original records.
- Queue track text is muted, unchecked rows are dimmed, and Queue/Missing Releases use a neutral selection highlight.

### Dashboards and audit surfaces

- Library Health contains local tag/organisation cards plus MQA Audit and Local Consolidation.
- Fix Library is a separate selectable dashboard with Missing Tags, Artwork and Online Replacements cards using the same card component.
- Audit tables expose status, evidence and intended destination/action, with extended row selection and checkbox selection. Batch consolidation verifies replacements first and presents the exact selected actions before filesystem changes. Overlapping source/destination batch operations are rejected. Remote replacements are queued first; originals remain until a separate verified consolidation.
- Existing optimization caches remain fingerprinted and are reused until library/catalogue inputs change. Completing consolidations invalidates related views and saves the remaining opportunities.

Manual batch match selection also requires each sibling to supply its own candidate recording ID. The old fallback could copy the first track’s option onto a sibling lacking that edition; it is removed. A failed/stale save is reported as unchanged.

### Shared counts

`link_statistics` is the shared indexed selector. It reads local inventory and market-specific durable links in bulk, never audio files.

- A linked track has both release and track IDs from a current durable association, or an existing tag pair when no durable decision supersedes it.
- A completely linked local release has links for **all locally present files** in its release group. This says nothing about whether all remote album tracks are downloaded.
- A partially linked release has some but not all local files linked.
- Deleted/missing files are excluded. Explicit durable unlink decisions override old ID tags.
- Overview and the Link Releases footer use the same all-libraries selector. The footer states that scope and reports both release and track units, rather than comparing track totals with album totals.

### Menus, file explorer and provider wording

`context_actions` supplies the same native menu prefix: Show in Finder/File Manager, Copy Path, View Metadata, Choose Match. Unavailable actions are disabled; contextual actions follow a separator. Applied to Queue, Missing Releases, Link Releases and audit/optimization tables. Metadata inspection uses the captured row data without another tag scan.

Finder now calls the actual shared function, converts file URIs through `QUrl`, preserves literal percent characters in ordinary paths, falls back to an existing containing directory for missing files, and launches the OS bridge without waiting on the UI thread.

UI labels use generic online/source terminology. Backend module names, API URLs, persisted settings namespaces and actual file tag identifiers remain unchanged for compatibility.

## 4. Code map

| Module | Responsibility |
| --- | --- |
| `app/library_manager/ui.py` | Queue cascade/group sorting, Missing Releases draft selection/batch queue, match diagnostics, dashboards, sorted-action identity, Finder |
| `app/library_manager/virtual_table.py` | Persistent hierarchical sort and checkbox roles |
| `app/library_manager/context_actions.py` | Shared native context-menu prefix and captured metadata inspection |
| `app/library_manager/link_statistics.py` | Common indexed link totals and active-ID selector |
| `app/library_manager/view_data.py` | Supplies shared statistics to Overview |
| `app/library_manager/tag_review.py` | N-ary edition evaluation, conflict-safe equivalence/cross-fill, position evidence |
| `app/library_manager/linking.py` | Versioned reconsideration, padding-compatible baselines, stale association cleanup |
| `app/library_manager/audit_pages.py` | Transparent audit columns, selection, reviewed batch operations |
| `app/library_manager/maintenance.py`, `library_workflows.py` | Validated local track/disc padding |
| `app/library_manager/enrichment.py`, `download_metadata.py` | Padded proposed/download tags |
| `app/tests/test_selection_and_identity.py` | Regression tests for this update |

Validation results and outstanding release checks are recorded in [RELEASE_CHECK.md](RELEASE_CHECK.md). A passing isolated suite demonstrates the tested behaviours; it is not a guarantee that all live provider metadata is correct or that every possible desktop interaction is defect-free.
