from __future__ import annotations

import csv
import io
import json
import os
import re
import sqlite3
import time
import unicodedata
from collections import defaultdict
from contextlib import contextmanager, closing
from datetime import datetime, timezone
from pathlib import Path

AUDIO = {'.flac', '.m4a', '.mp4', '.alac', '.mp3', '.aif', '.aiff', '.aifc', '.wav', '.wave'}


def now():
    return datetime.now(timezone.utc).isoformat()


def norm(value):
    return ' '.join(re.sub(r'[^\w\s]', ' ', unicodedata.normalize('NFKC', str(value)).casefold()).split())


def is_compilation_artist(value):
    return norm(value) in {'various artists', 'various artist', 'va', 'v a'}


def title_key(value):
    # Preserve meaningful symbol-only titles (e.g. $$$); do not equate them
    # with missing titles or other punctuation-only releases.
    return norm(value) or unicodedata.normalize('NFKC', str(value)).strip().casefold()


def base_title(value):
    return title_key(re.sub(r'\s*[\[(].*(?:deluxe|remaster|expanded|anniversary).*?[\])]', '', value, flags=re.I))


class Store:
    def __init__(self, path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.connect() as db:
            db.execute('PRAGMA journal_mode=WAL')
            version = db.execute('PRAGMA user_version').fetchone()[0]
            tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if version > 1 or ('libraries' in tables and 'local_files' not in tables):
                raise ValueError('Use a new app database; import organizer catalogues through the import action.')
            db.executescript('''
                CREATE TABLE IF NOT EXISTS roots(root TEXT PRIMARY KEY, scanned_at TEXT, status TEXT);
                CREATE TABLE IF NOT EXISTS local_files(path TEXT PRIMARY KEY, root TEXT, size INTEGER,
                    mtime INTEGER, metadata TEXT, error TEXT, present INTEGER DEFAULT 1);
                CREATE TABLE IF NOT EXISTS scans(id INTEGER PRIMARY KEY, root TEXT, started TEXT, ended TEXT,
                    status TEXT, summary TEXT);
                CREATE TABLE IF NOT EXISTS match_reviews(artist TEXT PRIMARY KEY, status TEXT, payload TEXT, error TEXT, updated TEXT);
                CREATE TABLE IF NOT EXISTS app_preferences(key TEXT PRIMARY KEY, payload TEXT);
                CREATE TABLE IF NOT EXISTS mappings(artist TEXT PRIMARY KEY, tidal_id TEXT, status TEXT,
                    evidence TEXT, manual INTEGER DEFAULT 0);
                CREATE TABLE IF NOT EXISTS additional_mappings(artist TEXT, tidal_id TEXT, PRIMARY KEY(artist,tidal_id));
                CREATE TABLE IF NOT EXISTS catalogue(artist_id TEXT, market TEXT, payload TEXT, fetched TEXT,
                    PRIMARY KEY(artist_id,market));
                CREATE TABLE IF NOT EXISTS track_links(path TEXT, market TEXT, stamp TEXT, payload TEXT,
                    PRIMARY KEY(path,market));
                CREATE TABLE IF NOT EXISTS favourite_artists(cache_id TEXT PRIMARY KEY, payload TEXT, fetched TEXT);
                CREATE TABLE IF NOT EXISTS queue(id TEXT PRIMARY KEY, payload TEXT, approved INTEGER DEFAULT 0,
                    decision TEXT DEFAULT 'queued', updated TEXT);
                CREATE TABLE IF NOT EXISTS ignored_local_files(path TEXT PRIMARY KEY, ignored_at TEXT);
                PRAGMA user_version=1;
            ''')

    @contextmanager
    def connect(self):
        db = sqlite3.connect(self.path, timeout=30)
        db.row_factory = sqlite3.Row
        try:
            with db:
                yield db
        finally:
            db.close()

    def rows(self, sql, args=()):
        with self.connect() as db:
            return [dict(r) for r in db.execute(sql, args)]

    def tracks(self):
        return [dict(json.loads(r['metadata']), path=r['path'], error=r['error']) for r in
                self.rows('SELECT * FROM local_files WHERE present=1 AND metadata IS NOT NULL')]

    def artists(self, include_compilations=False):
        result = defaultdict(list)
        for track in self.tracks():
            artist = track.get('artist') or 'Unknown artist'
            if include_compilations or not is_compilation_artist(artist):
                result[artist].append(track)
        return dict(sorted(result.items(), key=lambda p: p[0].casefold()))

    def mapping(self, artist, tidal_id, status, evidence, manual=False, extra_ids=()):
        with self.connect() as db:
            old = db.execute('SELECT manual FROM mappings WHERE artist=?', (artist,)).fetchone()
            if old and old[0] and not manual:
                return
            db.execute('INSERT OR REPLACE INTO mappings VALUES(?,?,?,?,?)',
                       (artist, tidal_id, status, json.dumps(evidence), int(manual)))
            db.execute('DELETE FROM additional_mappings WHERE artist=?', (artist,))
            db.executemany('INSERT OR IGNORE INTO additional_mappings VALUES(?,?)',
                           [(artist,str(ident)) for ident in extra_ids if str(ident) != str(tidal_id)])

    def linked_mappings(self):
        rows = self.rows("SELECT * FROM mappings WHERE status IN ('confirmed','auto')")
        extras = defaultdict(list)
        for row in self.rows('SELECT * FROM additional_mappings'):
            extras[row['artist']].append(row['tidal_id'])
        return [dict(row, tidal_id=ident) for row in rows
                for ident in [row['tidal_id']] + extras[row['artist']] if ident and not is_compilation_artist(row['artist'])]


    def preferences(self, key, defaults=None):
        rows = self.rows('SELECT payload FROM app_preferences WHERE key=?', (key,))
        return dict(defaults or {}, **(json.loads(rows[0]['payload']) if rows else {}))

    def save_preferences(self, key, values):
        with self.connect() as db:
            db.execute('INSERT OR REPLACE INTO app_preferences VALUES(?,?)', (key,json.dumps(values)))

    def match_preferences(self):
        defaults = dict(enabled=True, threshold=75, margin=25)
        rows = self.rows("SELECT payload FROM app_preferences WHERE key='matching'")
        if rows:
            defaults.update(json.loads(rows[0]['payload']))
        return defaults

    def save_match_preferences(self, preferences):
        with self.connect() as db:
            db.execute("INSERT OR REPLACE INTO app_preferences VALUES('matching',?)", (json.dumps(preferences),))

    def save_match_review(self, name, status, candidates, market, error=''):
        with self.connect() as db:
            db.execute('INSERT OR REPLACE INTO match_reviews VALUES(?,?,?,?,?)',
                       (name, status, json.dumps(dict(candidates=candidates, market=market)), error, now()))

    def cache(self, artist, market):
        rows = self.rows('SELECT * FROM catalogue WHERE artist_id=? AND market=?', (artist, market))
        return json.loads(rows[0]['payload']) if rows else None

    def save_catalogue(self, artist, market, payload):
        with self.connect() as db:
            previous = db.execute('SELECT payload FROM catalogue WHERE artist_id=? AND market=?', (artist, market)).fetchone()
            cached = {str(r['id']): r for r in json.loads(previous[0]).get('releases', [])} if previous else {}
            releases = []
            for release in payload.get('releases', []):
                saved = cached.get(str(release['id']), {})
                # Catalogue listings do not verify links. Keep the latest explicit
                # availability check for this release and market across refreshes.
                if (saved.get('link_checked_at') or 0) > (release.get('link_checked_at') or 0):
                    release = dict(release, available=saved.get('available'), link_checked_at=saved['link_checked_at'])
                releases.append(release)
            payload = dict(payload, releases=releases)
            db.execute('INSERT OR REPLACE INTO catalogue VALUES(?,?,?,?)',
                       (artist, market, json.dumps(payload), now()))

    def favourites(self, cache_id):
        rows = self.rows('SELECT * FROM favourite_artists WHERE cache_id=?', (cache_id,))
        return rows[0] if rows else None

    def save_favourites(self, cache_id, artists):
        with self.connect() as db:
            db.execute('INSERT OR REPLACE INTO favourite_artists VALUES(?,?,?)', (cache_id, json.dumps(artists), now()))

    def ignore_local_file(self, path):
        with self.connect() as db:
            db.execute('INSERT OR REPLACE INTO ignored_local_files VALUES(?,?)', (str(path), now()))

    def unignore_local_file(self, path):
        with self.connect() as db:
            db.execute('DELETE FROM ignored_local_files WHERE path=?', (str(path),))

    def set_local_files_ignored(self, paths, ignored=True):
        with self.connect() as db:
            if ignored:
                db.executemany('INSERT OR REPLACE INTO ignored_local_files VALUES(?,?)',[(str(path),now()) for path in set(paths)])
            else:
                db.executemany('DELETE FROM ignored_local_files WHERE path=?',[(str(path),) for path in set(paths)])

    def ignored_local_files(self):
        return {r['path'] for r in self.rows('SELECT path FROM ignored_local_files')}

    def enqueue(self, release, missing=None):
        if release.get('available') is not True:
            raise ValueError('Only releases with confirmed availability can be queued.')
        if missing is not None and not missing:raise ValueError('Select at least one track')
        payload = dict(release, selected_tracks=missing, url=f"https://tidal.com/browse/album/{release['id']}")
        with self.connect() as db:
            db.execute('INSERT INTO queue VALUES(?,?,0,?,?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,approved=0,decision=excluded.decision,updated=excluded.updated',
                       (str(release['id']), json.dumps(payload), 'queued', now()))

    def export(self, fmt):
        items = [json.loads(r['payload']) for r in self.rows(
            "SELECT * FROM queue WHERE approved=1 AND decision='queued' ORDER BY id")]
        records = []
        for item in items:
            # Track selections always export track URLs, never a whole partial album.
            if item.get('selected_tracks') is not None:
                for track in item['selected_tracks']:
                    records.append(dict(id=track['id'], kind='track', artist=item['artist'], title=track['title'],
                                        url=f"https://tidal.com/browse/track/{track['id']}"))
            else:
                records.append({k: item[k] for k in ('id', 'artist', 'title', 'url')} | {'kind': 'album'})
        if fmt == 'json':
            return json.dumps({'schema_version': 1, 'items': records}, indent=2, ensure_ascii=False)
        if fmt == 'csv':
            out = io.StringIO()
            writer = csv.DictWriter(out, fieldnames=['id', 'kind', 'artist', 'title', 'url'])
            writer.writeheader()
            # Spreadsheet formula protection for catalogue-controlled fields.
            writer.writerows({k: "'" + str(v) if str(v).startswith(('=', '+', '-', '@')) else v
                              for k, v in r.items()} for r in records)
            return out.getvalue()
        return ('#EXTM3U\n' if fmt == 'm3u' else '') + ''.join(r['url'] + '\n' for r in records)


def read_metadata(path):
    from .tag_io import open_audio
    audio = open_audio(path, pictures=False)
    if audio is None:
        raise ValueError('Unsupported or unreadable audio file')
    tags = audio.tags or {}
    def first(*keys):
        for key in keys:
            value = tags.get(key)
            if value:
                if key in ('albumartist','TPE2','artist','TPE1'):
                    values=value if isinstance(value,(list,tuple)) else getattr(value,'text',[value])
                    credit=', '.join(dict.fromkeys(str(v).strip() for v in values if str(v).strip()))
                    if credit:return credit
                    continue
                return str(value[0] if isinstance(value, (list, tuple)) else value)
        return ''
    # Easy tags cover FLAC/MP3/MP4. AIFF/WAV expose ID3 frames directly.
    return dict(artist=first('albumartist', 'TPE2', 'artist', 'TPE1') or 'Unknown artist', artist_grouping_version=3,
                track_artist=first('artist','TPE1'),
                album=first('album', 'TALB') or 'Unknown release', title=first('title', 'TIT2') or Path(path).stem,
                date=first('date', 'TDRC'), isrc=first('isrc', 'TSRC'),
                bpm=first('bpm','tempo','TBPM','tmpo'), musical_key=first('key','initialkey','TKEY'),
                track=first('tracknumber', 'TRCK'), tracktotal=first('tracktotal', 'totaltracks'),
                discnumber=first('discnumber', 'TPOS'), disctotal=first('disctotal', 'totaldiscs'),
                label=first('label', 'organization', 'publisher', 'TPUB'), releasetype=first('releasetype'), duration=round(audio.info.length, 3),
                copyright=first('copyright', 'TCOP'),
                tidal_track_id=first('tidal_track_id', 'tidaltrackid'),
                tidal_album_id=first('tidal_album_id', 'tidalalbumid'))


def scan(store, root, reader=read_metadata, cancelled=lambda: False, progress=lambda s: None, force=False):
    root = str(Path(root).expanduser().resolve())
    progress(f'Opening library · {root}')
    last_report = 0.0
    with store.connect() as db:
        db.execute('INSERT OR IGNORE INTO roots VALUES(?,NULL,?)', (root, 'not scanned'))
        scan_id = db.execute('INSERT INTO scans(root,started,status) VALUES(?,?,?)', (root, now(), 'running')).lastrowid
    counts = dict(read=0, unchanged=0, errors=0, missing=0)
    changed_artists=set()
    seen = set()
    restored = []
    status = 'complete'
    pending_writes = []
    def flush_index():
        if pending_writes:
            with store.connect() as db:
                db.executemany('INSERT OR REPLACE INTO local_files VALUES(?,?,?,?,?,?,1)', pending_writes)
            pending_writes.clear()
    try:
        if not Path(root).is_dir():
            raise OSError('Library is offline; saved snapshot retained')
        progress('Loading the saved file inventory for incremental comparison')
        previous = {r['path']: r for r in store.rows('SELECT * FROM local_files WHERE root=?', (root,))}
        progress(f'Loaded {len(previous):,} cached files · walking folders and comparing size and modification time')
        from .file_services import inventory
        entries, complete = inventory(root, AUDIO, cancelled, progress)
        if not complete:
            status = 'cancelled'
        for name, size, modified in entries:
            if cancelled():
                status = 'cancelled'
                break
            path = Path(name)
            seen.add(name)
            old = previous.get(name)
            old_metadata=json.loads(old['metadata'] or '{}') if old else {}
            old_grouping=reader is read_metadata and old and (old_metadata.get('artist_grouping_version')!=3 or 'track_artist' not in old_metadata)
            if not force and not old_grouping and old and (old['size'], old['mtime']) == (size, modified) and not old['error']:
                counts['unchanged'] += 1
                if not old['present']:
                    restored.append((name,))
                if len(seen) % 250 == 0:
                    progress(f'Checked {len(seen):,} files · {counts["unchanged"]:,} unchanged')
                continue
            metadata, error = None, None
            try:
                if counts['read'] == 0 or time.monotonic() - last_report >= .25:
                    progress(f'Reading audio tags · {path} · {len(seen):,} files checked')
                    last_report = time.monotonic()
                metadata = json.dumps(reader(path))
                current=json.loads(metadata);prior=json.loads(old['metadata'] or '{}') if old else {}
                if current.get('artist') and any(current.get(k)!=prior.get(k) for k in ('artist','album')):
                    changed_artists.add(current['artist'])
            except Exception:
                error = 'Metadata could not be read; retry on next scan'
                metadata = old['metadata'] if old else None
                counts['errors'] += 1
                progress(f'Could not read tags · {path} · will retry on the next scan')
            counts['read'] += 1
            pending_writes.append((name, root, size, modified, metadata, error))
            if len(pending_writes) >= 64:flush_index()
            if time.monotonic() - last_report >= .25:
                progress(f"Scanned {len(seen):,} files · {counts['read']:,} read · {counts['errors']} errors")
                last_report = time.monotonic()
        if cancelled():
            status = 'cancelled'
        if status == 'complete':
            progress('Folder traversal complete · updating the library index for removed or moved files')
            absent = set(previous) - seen
            counts['missing'] = sum(bool(previous[p]['present']) for p in absent)
            with store.connect() as db:
                db.executemany('UPDATE local_files SET present=0 WHERE path=?', [(p,) for p in absent])
    except OSError as exc:
        status = 'offline or incomplete'
        progress(str(exc))
    finally:
        flush_index()
        with store.connect() as db:
            db.executemany('UPDATE local_files SET present=1 WHERE path=?', restored)
            db.execute('UPDATE scans SET ended=?,status=?,summary=? WHERE id=?',
                       (now(), status, json.dumps(counts), scan_id))
            db.execute('UPDATE roots SET status=?,scanned_at=CASE WHEN ?=\'complete\' THEN ? ELSE scanned_at END WHERE root=?',
                       (status, status, now(), root))
    progress(f'Scan {status} · {counts["read"]:,} metadata reads · {counts["unchanged"]:,} unchanged · {counts["errors"]} errors · {counts["missing"]} removed from the library index')
    return dict(status=status, changed_artists=sorted(changed_artists), **counts)


def track_matches(local, remote):
    from .release_matching import recording_matches
    return not local.get('error') and recording_matches(local, remote)


def resolve(tracks, candidates):
    scored = []
    for artist in candidates:
        albums, matching_tracks, summary_matches = set(), set(), set()
        for release in artist['releases']:
            local = [t for t in tracks if title_key(t['album']) == title_key(release['title'])]
            if local and not release.get('tracks_loaded', bool(release['tracks'])):
                summary_matches.add(title_key(release['title']))
            hits = sum(any(track_matches(t, rt) for t in local) for rt in release['tracks'])
            if hits:
                albums.add(title_key(release['title']))
                matching_tracks.update((title_key(t['album']), title_key(t['title'])) for t in local
                                       if any(track_matches(t, rt) for rt in release['tracks']))
        matched = len(matching_tracks)
        score = max(min(80, len(summary_matches) * 25), min(98, len(albums) * 25 + min(matched, 12) * 6))
        scored.append(dict(artist=artist, score=score, independent_releases=len(albums), summary_releases=len(summary_matches), evidence=(f'{len(summary_matches)} local release titles match; track details not checked' if summary_matches and not albums else f'{len(albums)} releases and {matched} track titles/identifiers match')))
    scored.sort(key=lambda x: x['score'], reverse=True)
    accepted = bool(scored and scored[0]['score'] >= 85 and scored[0]['independent_releases'] >= 2 and
                    (len(scored) == 1 or scored[0]['score'] - scored[1]['score'] >= 25))
    return scored, accepted


def coverage(tracks, artist):
    results = []
    for release in artist['releases']:
        exact = [t for t in tracks if (str(t['tidal_album_id']) == str(release['id']) if t.get('tidal_album_id')
                 else title_key(t['album']) == title_key(release['title']))]
        related = exact or [t for t in tracks if base_title(t['album']) == base_title(release['title'])]
        loc_paths = [t['path'] for t in (exact or related) if t.get('path')]
        if not release.get('tracks_loaded', bool(release['tracks'])):
            state = 'Present locally' if exact else ('Alternate edition' if related else 'Missing release')
            if release.get('available') is False:
                state = 'Unavailable'
            reason = ('Local release title/ID found; completeness not checked' if exact else
                      'Related local edition found; track overlap not checked' if related else 'No matching local release title/ID; track details not checked')
            results.append(dict(release=release, state=state, missing=[], reason=reason, summary=True, local_tracks=loc_paths))
            continue
        from .release_matching import structure
        info = structure(related, release) if related else None
        missing = info['missing'] if info and info['compatible'] else [rt for rt in release['tracks'] if not any(track_matches(t, rt) for t in related)]
        hits = len(release['tracks']) - len(missing)
        if release.get('available') is False:
            state = 'Unavailable'
        elif not release['tracks'] or release.get('available') is None:
            state = 'Needs review'
        elif info and any(c in ('Track total differs','Disc total differs') for c in info['conflicts']) and hits:
            state = 'Alternate edition'
        elif related and not exact and hits:
            state = 'Alternate edition'
        elif hits == len(release['tracks']):
            state = 'Owned complete'
        elif hits:
            state = 'Owned partial'
        elif related:
            state = 'Needs review'
        else:
            state = 'Missing release'
        results.append(dict(release=release, state=state, missing=missing,
                            reason=f'{hits}/{len(release["tracks"])} tracks matched in local release',
                            local_tracks=loc_paths, incomplete_local=bool(info and info['incomplete']),
                            structure=info))
    return results


def import_legacy(store, source):
    """Copy existing organizer snapshots without opening the source for writes."""
    with closing(sqlite3.connect(Path(source).resolve().as_uri() + '?mode=ro', uri=True)) as old:
        old.row_factory = sqlite3.Row
        roots = list(old.execute('SELECT * FROM libraries'))
        files = list(old.execute('SELECT * FROM files'))
        tags = defaultdict(dict)
        for row in old.execute('SELECT * FROM tags'):
            tags[row['file_id']][row['tag_key'].casefold()] = json.loads(row['value_json'])
    def value(data, *keys):
        for key in keys:
            v = data.get(key)
            if isinstance(v, dict):
                v = v.get('text', '')
            if isinstance(v, list):
                v = v[0] if v else ''
            if v:
                return str(v)
        return ''
    with store.connect() as db:
        for root in roots:
            db.execute('INSERT OR IGNORE INTO roots VALUES(?,?,?)', (root['root'], root['scanned_at'], 'imported; rescan recommended'))
        for f in files:
            t = tags[f['id']]
            metadata = dict(artist=value(t, 'albumartist', 'tpe2', 'aart', 'artist', 'tpe1', '©art') or 'Unknown artist',
                            album=value(t, 'album', 'talb', '©alb') or 'Unknown release',
                            title=value(t, 'title', 'tit2', '©nam') or Path(f['path']).stem,
                            date=value(t, 'date', 'tdrc', '©day'), isrc=value(t, 'isrc', 'tsrc'))
            path = Path(f['path'])
            if not path.is_absolute():
                path = Path(f['library_root']) / path
            db.execute('INSERT OR IGNORE INTO local_files VALUES(?,?,?,?,?,?,1)',
                       (str(path), f['library_root'], f['size_bytes'], None, json.dumps(metadata), f['error']))
    return len(files)
