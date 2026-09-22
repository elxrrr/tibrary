# Release checks

Last recorded application validation: **21 September 2026**. This is a tested source build, not a signed/notarised distribution or a guarantee of live provider availability.

## Recorded verification

- Full isolated regression suite: **256 tests passed** (137.9 seconds).
- Subsequent focused release/selection/consolidation checks: **48 passed**.
- Native macOS UI/selection/consolidation run: **15 passed** (7.9 seconds).
- Later manual-match isolation and linked-view checks: **19 passed** (10.1 seconds).
- Final selection/identity confirmation: **11 passed** (1.8 seconds).
- Application module compilation passed; native Health and Fix Library screenshots were inspected.

These runs overlap; their counts are not additive. The full-suite run preceded the final refinements, which were covered by the later focused/native runs.

Coverage included whole-release/edition matching, incomplete albums, equivalent IDs, conflicting artist metadata, numeric padding, parent/child approval, selection across filters, sorting, shared counters, Finder paths, guarded file operations, cache reuse and native navigation. The [architecture audit](LINKING_AND_UI_AUDIT.md) explains the implemented rules and their limits.

Tests used temporary files, databases and isolated settings, with mocked provider responses. The runner blocks external-volume mutations and external networking. No NVME music files, production link caches or credentials were changed by this validation. Earlier offline adapter compatibility checks also passed; those do not establish compatibility with future upstream commits.

## Remaining release milestones

- Validate metadata and artwork for a known recording against a connected live account.
- Perform an explicitly approved download to a disposable directory; inspect tags, audio quality, artwork and output paths.
- Exercise a real streaming-component update and rollback. Mocked build tests and existing offline adapter checks do not replace live acceptance.
- Check a clean installation and representative large-library performance.
- Complete packaging, signing/notarisation and distribution checks before publishing a macOS release.
- Perform native validation before claiming support for other operating systems.

## Repeating verification

Commands and isolation rules are in [Development and setup](DEVELOPMENT.md#tests). Prefer focused tests for routine changes; reserve full-suite and live-library acceptance work for release milestones. Do not modify production music as part of development verification.

## Documentation maintenance

This file is the single release-status record. Update it when validation or remaining milestones change. Historical implementation walkthroughs have been consolidated into the current user/developer guides and the architecture audit; the original product brief remains under `reference/`.
