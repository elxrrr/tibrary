# Development and setup

See the [user guide](../README.md) for current workflows and the [linking audit](LINKING_AND_UI_AUDIT.md) for matching algorithms, scoring and UI state ownership.

## Application setup

From the application root (`source_app`), with Python 3.12 or newer:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -e ./app
./Start.command
```

An existing workspace may already contain `.venv`. The launcher resolves its own directory, independently of the terminal's current directory.

For the separate fictional library, double-click `app/tools/Demo.command` or run:

```sh
./app/tools/Demo.command
```

Demo mode disables real-library writes/downloads. To install the optional bundled download runtime, install Python 3.13 and run:

```sh
"./app/tools/Setup downloads.command"
```

This creates `app/resources/tidaler/.venv`. Streaming components can subsequently be managed in the application's Settings; updates are built separately, checked and activated with rollback support.

## Source layout

| Location | Purpose |
| --- | --- |
| `Start.command` | Normal application launcher |
| `app/tools/` | Demo and optional download setup commands |
| `app/library_manager/` | Application implementation |
| `app/tests/` | Automated tests and UI smoke scripts |
| `app/docs/` | Current user, developer, architecture and release documentation |
| `app/docs/reference/` | Original historical product brief |
| `.venv/` | Application Python runtime |
| `app/resources/tidaler/`, `app/resources/python-tidal/` | Upstream source repositories, Git histories and licences |
| `app/resources/backends/` | Managed backend builds, created on update |

The [architecture code map](LINKING_AND_UI_AUDIT.md#4-code-map) identifies the modules responsible for tables, matching, statistics and file operations. Catalogue access, browser account authorization and credentials live in `tidal.py`, `account.py` and `credentials.py`. Downloads and extended metadata use isolated bridge processes. Long-running work belongs in cancellable workers, with queued delivery to the GUI thread.

## Data and credentials

The normal catalogue is `~/Library/Application Support/Tibrary/library.sqlite3`; demo uses `demo.sqlite3`. `--db` overrides the database location. SQLite uses transactions and WAL; close the app before copying its database. Legacy organizer databases are imported read-only.

Client credentials and remembered official-API account tokens use the OS credential store. Session-only overrides do not replace saved credentials. Environment fallbacks are `TIDAL_CLIENT_ID`, `TIDAL_CLIENT_SECRET` and `TIDAL_MARKET` (default GB). Secrets are masked/redacted.

The catalogue account uses PKCE, checked state and `http://127.0.0.1:8765/callback`. Its developer application needs `collection.read` and `search.read`; an older collection-only grant needs reconnection for search. Client authentication alone does not establish catalogue/collection authorization.

The subscriber/download session is separately stored under the app data directory's `downloader-session` with restricted permissions. It does not receive the catalogue client secret. Startup checks saved sessions without initiating browser sign-in or downloads.

## Safety and consistency

- Local grouping uses the complete Album Artist tag, falling back to Track Artist only when absent. Compilation tracks remain in totals even when Various Artists is excluded from artist discovery.
- Linking persists associations and evidence, never file mutations. Shared statistics and identity rules are documented in the linking audit.
- Replanning reuses inspected tags. Before applying file changes, fingerprints must still agree; stale sources, symlinks and collisions are rejected.
- Tag/artwork writes verify audio and expected metadata before publication. Same-volume moves preserve file bytes; verified-copy fallback handles unsupported or cross-volume operations.
- No persistent backup/quarantine copies are created. Destructive consolidation uses Trash and explicit review. Unrelated files and sidecars remain protected.
- Applied changes update the index and invalidate affected linking state. There is no filesystem watcher; external changes require an incremental or explicit full tag refresh.
- Existing DJ analysis is protected; lyrics are excluded from acquisition. Unsupported or conflicting metadata must not be invented.
- User-facing wording is provider-neutral; backend identifiers, credentials, tag names and API addresses retain their actual names.

## Tests

From the application root, run focused modules for changed behaviour:

```sh
.venv/bin/python app/tests/run_isolated.py test_selection_and_identity
```

At a release milestone:

```sh
.venv/bin/python app/tests/run_isolated.py
```

For native macOS checks (requires a graphical session):

```sh
TIBRARY_NATIVE_TEST=1 .venv/bin/python app/tests/run_isolated.py test_navigation_and_consolidation
```

The isolated runner uses temporary settings, credential storage, databases and music fixtures. It blocks external networking and writes to external volumes. Use mocked provider responses for routine testing; do not run mutation tests against a user's library.

See [Release checks](RELEASE_CHECK.md) for recorded results and the outstanding live-account/distribution milestones. Dependency references and attribution are in [THIRD_PARTY.md](../THIRD_PARTY.md).
