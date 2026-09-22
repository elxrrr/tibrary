import io
import json
import sqlite3
import tempfile
import time
import unittest
from contextlib import closing
from pathlib import Path
from urllib.error import HTTPError

from library_manager.core import Store, scan, resolve, coverage, import_legacy
from library_manager.demo import catalogue
from library_manager.tidal import Tidal, CatalogueError


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.store = Store(self.base / 'app.sqlite3')
        self.artist = catalogue()

    def local(self):
        return [dict(t, artist=self.artist['name'], album=r['title'])
                for r in self.artist['releases'][:2] for t in r['tracks']]

    def test_incremental_offline_changed_cancelled_and_missing(self):
        root = self.base / 'music'; root.mkdir()
        audio = root / 'track.flac'; audio.write_bytes(b'not real audio')
        calls = []
        def reader(path):
            calls.append(path)
            return self.local()[0]
        self.assertEqual(scan(self.store, root, reader)['read'], 1)
        self.assertEqual(scan(self.store, root, reader)['unchanged'], 1)
        self.assertEqual(len(calls), 1)
        audio.write_bytes(b'changed audio data')
        self.assertEqual(scan(self.store, root, reader)['read'], 1)
        audio.unlink()
        scan(self.store, root, reader, cancelled=lambda: True)
        # An empty cancelled scan must not mark unseen files missing.
        self.assertEqual(len(self.store.tracks()), 1)
        root.rmdir()
        self.assertEqual(scan(self.store, root, reader)['status'], 'offline or incomplete')
        self.assertEqual(len(self.store.tracks()), 1)
        root.mkdir(); scan(self.store, root, reader)
        self.assertEqual(len(self.store.tracks()), 0)

    def test_name_only_and_ambiguous_candidates_stay_in_review(self):
        blank = dict(id='2', name=self.artist['name'], releases=[])
        _, accepted = resolve(self.local(), [blank]); self.assertFalse(accepted)
        _, accepted = resolve(self.local(), [self.artist, self.artist]); self.assertFalse(accepted)
        scored, accepted = resolve(self.local(), [self.artist, blank]); self.assertTrue(accepted)
        self.assertIn('2 releases', scored[0]['evidence'])

    def test_one_release_cannot_auto_resolve_even_with_many_tracks(self):
        import copy
        artist = copy.deepcopy(self.artist)
        artist['releases'] = [artist['releases'][0]]
        artist['releases'][0]['tracks'] *= 8
        self.assertFalse(resolve(self.local(), [artist])[1])

    def test_real_wav_scan_does_not_modify_audio(self):
        import wave
        import hashlib
        from library_manager.core import read_metadata
        from mutagen.wave import WAVE
        from mutagen.id3 import TIT2, TPE2, TALB
        root = self.base / 'wav'; root.mkdir(); path = root / 'sample.wav'
        with wave.open(str(path), 'wb') as audio:
            audio.setnchannels(1); audio.setsampwidth(2); audio.setframerate(8000)
            audio.writeframes(b'\0' * 16000)
        audio = WAVE(path); audio.add_tags()
        audio.tags.add(TIT2(encoding=3, text=['Sample']))
        audio.tags.add(TPE2(encoding=3, text=['Test Artist']))
        audio.tags.add(TALB(encoding=3, text=['Test Release'])); audio.save()
        before = hashlib.sha256(path.read_bytes()).digest()
        result = scan(self.store, root)
        self.assertEqual(result['errors'], 0)
        self.assertEqual(self.store.tracks()[0]['artist'], 'Test Artist')
        self.assertEqual(scan(self.store, root)['unchanged'], 1)
        self.assertEqual(hashlib.sha256(path.read_bytes()).digest(), before)

    def test_manual_unlink_survives_resolution(self):
        self.store.mapping('Artist', None, 'unlinked', 'Manual choice', True)
        self.store.mapping('Artist', '123', 'auto', 'Candidate')
        row = self.store.rows('SELECT * FROM mappings')[0]
        self.assertEqual(row['status'], 'unlinked'); self.assertIsNone(row['tidal_id'])

    def test_coverage_complete_partial_alternate_missing_unavailable(self):
        local = self.local()[:-1]
        states = {r['release']['title']: r['state'] for r in coverage(local, self.artist)}
        self.assertEqual(states['Blue Hours'], 'Owned complete')
        self.assertEqual(states['Night Maps'], 'Owned partial')
        self.assertEqual(states['Blue Hours (Deluxe)'], 'Alternate edition')
        self.assertEqual(states['Glass'], 'Missing release')
        self.assertEqual(states['Private Weather'], 'Unavailable')

    def test_queue_restart_dedup_approval_and_partial_export(self):
        release = self.artist['releases'][1]
        missing = release['tracks'][-1:]
        self.store.enqueue(release, missing); self.store.enqueue(release, missing)
        self.assertEqual(self.store.export('txt'), '')
        with self.store.connect() as db: db.execute('UPDATE queue SET approved=1')
        reopened = Store(self.store.path)
        self.assertEqual(len(reopened.rows('SELECT * FROM queue')), 1)
        self.assertEqual(reopened.export('txt'), f"https://tidal.com/browse/track/{missing[0]['id']}\n")
        self.assertEqual(json.loads(reopened.export('json'))['items'][0]['kind'], 'track')
        with self.assertRaises(ValueError): reopened.enqueue(self.artist['releases'][-1])

    def test_legacy_import_preserves_source_and_offline_snapshot(self):
        source = self.base / 'legacy.sqlite3'
        with closing(sqlite3.connect(source)) as db:
            db.executescript('CREATE TABLE libraries(root,scanned_at,stale); CREATE TABLE files(id,library_root,path,size_bytes,mtime_ns,error); CREATE TABLE tags(file_id,tag_key,value_json);')
            db.execute("INSERT INTO libraries VALUES('/offline','2020',1)")
            db.execute("INSERT INTO files VALUES(1,'/offline','song.flac',123,456,NULL)")
            db.executemany('INSERT INTO tags VALUES(1,?,?)', [('albumartist', '["Artist"]'), ('title', '["Song"]')])
            db.commit()
        before = source.read_bytes()
        self.assertEqual(import_legacy(self.store, source), 1)
        self.assertEqual(source.read_bytes(), before)
        self.assertEqual(self.store.tracks()[0]['artist'], 'Artist')
        import_legacy(self.store, source)
        self.assertEqual(len(self.store.tracks()), 1)


class ApiTests(unittest.TestCase):
    def api(self, responses):
        calls = []
        def transport(request, **kwargs):
            calls.append(request.full_url)
            response = responses.pop(0)
            if isinstance(response, Exception): raise response
            return io.BytesIO(json.dumps(response).encode())
        api = Tidal(transport=transport); api.token='test'; api.expires=time.monotonic()+1000
        return api, calls

    def test_search_pagination_and_rate_limit(self):
        entity = lambda i: dict(type='artists', id=i, attributes={'name': 'Same Name'})
        api, calls = self.api([
            {'data': [{'type': 'searchResults', 'id': 'opaque-result'}]},
            HTTPError('https://openapi.tidal.com/v2/searchResults/a', 429, 'slow', {'Retry-After': '0'}, None),
            {'data': [{'type':'artists','id':'1'}], 'included':[entity('1')], 'links':{'next':'?page%5Bcursor%5D=two&countryCode=GB'}},
            {'data': [entity('2')], 'links': {}}])
        self.assertEqual([a['id'] for a in api.search('a')], ['1','2'])
        self.assertEqual(len(calls),4)

    def test_artist_albums_and_track_pages(self):
        api, calls = self.api([
            {'data': {'attributes': {'name':'Artist'}}},
            {'data': [{'type':'albums','id':'10','attributes':{'title':'Album','numberOfItems':2,'availability':['STREAM']}}]},
            {'data': [{'type':'tracks','id':'11','attributes':{'title':'One','duration':'PT3M'}}], 'links':{'next':'?page%5Bcursor%5D=two'}},
            {'data': [{'type':'tracks','id':'12','attributes':{'title':'Two','duration':'PT2M30S'}}]}])
        result = api.artist('1', detailed=True)
        self.assertEqual(len(result['releases'][0]['tracks']), 2)
        self.assertEqual(result['releases'][0]['tracks'][1]['duration'],150)
        self.assertTrue(result['releases'][0]['available'])

    def test_reject_external_pagination_without_sending_token(self):
        api, calls = self.api([{'data':[{'id':'opaque-result'}]}, {'data':[], 'links':{'next':'https://evil.example/steal'}}])
        with self.assertRaises(CatalogueError): api.search('a')
        self.assertEqual(len(calls),2)

    def test_failed_refresh_does_not_return_partial_catalogue(self):
        api, _ = self.api([{'data':[{'id':'opaque-result'}]}, {'data':[], 'links':{'next':'?page=2'}},
                          HTTPError('https://openapi.tidal.com/v2/a',403,'forbidden',{},None)])
        with self.assertRaises(CatalogueError): api.search('a')


if __name__ == '__main__': unittest.main()
