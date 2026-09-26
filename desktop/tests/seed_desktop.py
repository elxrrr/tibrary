"""Disposable real-file fixture for desktop integration tests, with zero external dependencies."""
import json
import os
import sqlite3
import sys
from pathlib import Path


def write_flac(path, artist, album, title, track, total):
    path.parent.mkdir(parents=True, exist_ok=True)
    # Minimal valid FLAC container
    stream = (4096).to_bytes(2, 'big') * 2 + b'\0' * 6 + ((8000 << 44) | (15 << 36) | 8000).to_bytes(8, 'big') + b'\0' * 16
    tags = {'ARTIST': artist, 'ALBUMARTIST': artist, 'ALBUM': album, 'TITLE': title, 'TRACKNUMBER': f'{track}/{total}', 'DISCNUMBER': '1/1', 'DATE': '2020-04-03'}
    items=[f'{k}={v}'.encode() for k,v in tags.items()]
    comments=(0).to_bytes(4,'little')+len(items).to_bytes(4,'little')+b''.join(len(v).to_bytes(4,'little')+v for v in items)
    payload=b'fLaC'+b'\x00\x00\x00\x22'+stream+b'\x84'+len(comments).to_bytes(3,'big')+comments+b'audio payload'
    path.write_bytes(payload)


def catalogue():
    def release(ident, title, names, date, available=True, kind='ALBUM'):
        return dict(
            id=str(ident),
            artist='North Assembly',
            title=title,
            date=date,
            available=available,
            type=kind,
            quality='LOSSLESS',
            track_count=len(names),
            tracks_loaded=True,
            tracks=[
                dict(
                    id=str(ident * 100 + i),
                    title=n,
                    duration=180 + i * 10,
                    track_number=i + 1,
                    disc_number=1,
                    isrc=f"GB00{ident}{i}",
                )
                for i, n in enumerate(names)
            ],
        )

    return dict(
        id='900001',
        name='North Assembly',
        releases=[
            release(910001, 'Blue Hours', ['First Light', 'Drift', 'Low Tide'], '2020-04-03'),
            release(910002, 'Night Maps', ['Signal', 'Afterimage', 'Homeward'], '2022-09-16'),
            release(910003, 'Blue Hours (Deluxe)', ['First Light', 'Drift', 'Low Tide', 'Open Water'], '2023-04-03'),
            release(910004, 'Between Stations', ['Platform', 'Passing'], '2021-06-11', kind='EP'),
            release(910005, 'Distant Rooms', ['Threshold', 'Still Life'], '2025-02-07'),
            release(910006, 'Glass', ['Glass'], '2026-08-28', kind='SINGLE'),
            release(910007, 'Private Weather', ['Rain'], '2024-01-12', available=False),
        ],
    )


def main():
    root = Path(sys.argv[1]).resolve()
    library = root / 'music'
    db_file = root / 'db'
    db_file.parent.mkdir(parents=True, exist_ok=True)

    screenshot_mode = bool(os.environ.get('TIBRARY_SCREENSHOTS'))
    album_folder = library / 'North Assembly/Blue Hours (2020)' if screenshot_mode else library
    f1 = album_folder / 'First Light.flac'
    f2 = album_folder / 'Drift.flac'
    write_flac(f1, 'North Assembly', 'Blue Hours', 'First Light', 1, 3)
    write_flac(f2, 'North Assembly', 'Blue Hours', 'Drift', 2, 3)

    cat = catalogue()

    with sqlite3.connect(db_file) as conn:
        conn.execute("CREATE TABLE IF NOT EXISTS roots(root TEXT PRIMARY KEY, scanned_at TEXT, status TEXT)")
        conn.execute("CREATE TABLE IF NOT EXISTS local_files(path TEXT PRIMARY KEY, root TEXT, size INTEGER, mtime INTEGER, metadata TEXT, error TEXT, present INTEGER DEFAULT 1)")
        conn.execute("CREATE TABLE IF NOT EXISTS mappings(artist TEXT PRIMARY KEY, tidal_id TEXT, status TEXT, evidence TEXT, manual INTEGER DEFAULT 0)")
        conn.execute("CREATE TABLE IF NOT EXISTS additional_mappings(artist TEXT, tidal_id TEXT, PRIMARY KEY(artist,tidal_id))")
        conn.execute("CREATE TABLE IF NOT EXISTS catalogue(artist_id TEXT, market TEXT, payload TEXT, fetched TEXT, PRIMARY KEY(artist_id,market))")
        conn.execute("CREATE TABLE IF NOT EXISTS track_links(path TEXT, market TEXT, stamp TEXT, payload TEXT, PRIMARY KEY(path,market))")
        conn.execute("CREATE TABLE IF NOT EXISTS queue(id TEXT PRIMARY KEY, payload TEXT, approved INTEGER DEFAULT 0, decision TEXT DEFAULT 'queued', updated TEXT)")
        conn.execute("CREATE TABLE IF NOT EXISTS app_preferences(key TEXT PRIMARY KEY, payload TEXT)")
        conn.execute("CREATE TABLE IF NOT EXISTS ignored_local_files(path TEXT PRIMARY KEY, ignored_at TEXT)")

        conn.execute("INSERT OR REPLACE INTO roots VALUES (?, '2026-09-24T00:00:00Z', 'active')", (str(library),))

        meta1 = {
            'artist': ['North Assembly'],
            'albumartist': ['North Assembly'],
            'album': ['Blue Hours'],
            'title': ['First Light'],
            'tracknumber': ['1/3'],
            'discnumber': ['1/1'],
            'date': ['2020-04-03'],
            'duration': 180.0,
            'tidal_album_id': '910001',
            'tidal_track_id': '91000100',
        }
        meta2 = {
            'artist': ['North Assembly'],
            'albumartist': ['North Assembly'],
            'album': ['Blue Hours'],
            'title': ['Drift'],
            'tracknumber': ['2/3'],
            'discnumber': ['1/1'],
            'date': ['2020-04-03'],
            'duration': 190.0,
            'tidal_album_id': '910001',
            'tidal_track_id': '91000101',
        }

        conn.execute("INSERT OR REPLACE INTO local_files VALUES (?, ?, ?, ?, ?, NULL, 1)", (str(f1), str(library), f1.stat().st_size, f1.stat().st_mtime_ns, json.dumps(meta1)))
        conn.execute("INSERT OR REPLACE INTO local_files VALUES (?, ?, ?, ?, ?, NULL, 1)", (str(f2), str(library), f2.stat().st_size, f2.stat().st_mtime_ns, json.dumps(meta2)))

        if screenshot_mode:
            for path, album, title, track, total, duration in [
                (album_folder / 'Low Tide.flac', 'Blue Hours', 'Low Tide', 3, 3, 200.0),
                (library / 'North Assembly/First Light (2019)/First Light.flac', 'First Light', 'First Light', 1, 1, 180.0),
            ]:
                write_flac(path, 'North Assembly', album, title, track, total)
                metadata = {'albumartist':'North Assembly','artist':'North Assembly','album':album,'title':title,'tracknumber':str(track),'tracktotal':str(total),'discnumber':'1','duration':duration,'date':'2020-04-03'}
                conn.execute("INSERT OR REPLACE INTO local_files VALUES (?, ?, ?, ?, ?, NULL, 1)", (str(path), str(library), path.stat().st_size, path.stat().st_mtime_ns, json.dumps(metadata)))

        conn.execute("INSERT OR REPLACE INTO mappings VALUES ('North Assembly', '900001', 'confirmed', 'fixture', 1)")
        conn.execute("INSERT OR REPLACE INTO mappings VALUES ('Wrong', '900001', 'confirmed', 'fixture', 1)")

        conn.execute("INSERT OR REPLACE INTO catalogue VALUES ('900001', 'GB', ?, '2026-09-24')", (json.dumps(cat),))

        # Enqueue all available releases with approved=0
        for r in cat['releases']:
            if r['available']:
                conn.execute(
                    "INSERT OR REPLACE INTO queue VALUES (?, ?, 0, 'queued', '2026-09-24T00:00:00Z')",
                    (str(r['id']), json.dumps(r)),
                )


if __name__ == '__main__':
    main()
