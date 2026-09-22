"""Fictional offline fixtures, stored separately from a real library."""
import json
from .core import now


def catalogue():
    def release(ident, title, names, date, available=True, kind='ALBUM'):
        return dict(id=str(ident), artist='North Assembly', title=title, date=date, available=available, type=kind,
                    quality='LOSSLESS', tracks=[dict(id=str(ident * 100 + i), title=n, duration=180 + i * 10)
                                               for i, n in enumerate(names)])
    return dict(id='900001', name='North Assembly', releases=[
        release(910001, 'Blue Hours', ['First Light', 'Drift', 'Low Tide'], '2020-04-03'),
        release(910002, 'Night Maps', ['Signal', 'Afterimage', 'Homeward'], '2022-09-16'),
        release(910003, 'Blue Hours (Deluxe)', ['First Light', 'Drift', 'Low Tide', 'Open Water'], '2023-04-03'),
        release(910004, 'Between Stations', ['Platform', 'Passing'], '2021-06-11', kind='EP'),
        release(910005, 'Distant Rooms', ['Threshold', 'Still Life'], '2025-02-07'),
        release(910006, 'Glass', ['Glass'], '2026-08-28', kind='SINGLE'),
        release(910007, 'Private Weather', ['Rain'], '2024-01-12', available=False)])


def seed(store):
    if store.rows('SELECT * FROM roots'):
        return
    artist = catalogue()
    with store.connect() as db:
        db.execute('INSERT INTO roots VALUES(?,?,?)', ('Demo library (fictional)', now(), 'demo'))
        for release in artist['releases'][:2]:
            for track in release['tracks'][:3 if release['title'] == 'Blue Hours' else 2]:
                local = dict(track, artist=artist['name'], album=release['title'], date=release['date'])
                db.execute('INSERT INTO local_files VALUES(?,?,?,?,?,?,1)',
                           (f"demo/{release['title']}/{track['title']}.flac", 'Demo library (fictional)', 0, 0, json.dumps(local), None))
    store.save_catalogue(artist['id'], 'DEMO', artist)
    store.mapping(artist['name'], artist['id'], 'confirmed', 'Demo fixture: two matching releases', True)
