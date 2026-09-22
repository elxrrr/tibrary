import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from library_manager.tidal import Tidal, RequestPacer
from library_manager.core import Store, resolve, coverage


class SummaryTests(unittest.TestCase):
    def summary(self, ident='1', title='Local album'):
        return dict(id=ident, artist='Artist',title=title,date='2024-01-01',type='ALBUM',available=True,tracks=[],tracks_loaded=False,track_count=10)

    def test_fifteen_release_catalogue_uses_two_requests_no_tracks(self):
        calls=[]
        responses=[{'data':{'attributes':{'name':'Artist'}}}, {'data':[
            {'id':str(i),'type':'albums','attributes':{'title':f'Album {i}','numberOfItems':10,'albumType':'ALBUM'}} for i in range(15)]}]
        def transport(request, **kwargs):
            calls.append(request.full_url)
            return io.BytesIO(json.dumps(responses.pop(0)).encode())
        api=Tidal(transport=transport);api.token='test';api.expires=float('inf')
        artist=api.artist('1')
        self.assertEqual(len(artist['releases']),15)
        self.assertEqual(len(calls),2)
        self.assertTrue(all(not r['tracks_loaded'] for r in artist['releases']))
        self.assertFalse(any('/albums/' in url for url in calls))

    def test_local_release_presence_does_not_claim_track_completeness(self):
        local=[dict(album='Local album',title='Only one local track')]
        result=coverage(local,dict(releases=[self.summary(),self.summary('2','New album')]))
        self.assertEqual(result[0]['state'],'Present locally')
        self.assertEqual(result[1]['state'],'Missing release')
        self.assertTrue(result[0]['summary']);self.assertIn('not checked',result[0]['reason'])

    def test_multiple_summary_candidates_remain_manual(self):
        tracks=[dict(album=f'Album {i}',title='Track') for i in range(6)]
        artists=[dict(id=str(i),name='Artist',releases=[self.summary(str(j),f'Album {j}') for j in range(6)]) for i in range(2)]
        scores,auto=resolve(tracks,artists)
        self.assertFalse(auto);self.assertEqual(len(scores),2);self.assertGreater(scores[0]['score'],0)
        self.assertIn('track details not checked',scores[0]['evidence'])

    def test_selected_release_detail_only_and_partial_coverage(self):
        calls=[]
        def transport(request, **kwargs):
            calls.append(request.full_url)
            return io.BytesIO(json.dumps({'data':[
                {'type':'tracks','id':'11','attributes':{'title':'One','duration':'PT3M'}},
                {'type':'tracks','id':'12','attributes':{'title':'Two','duration':'PT3M'}}]}).encode())
        api=Tidal(transport=transport);api.token='test';api.expires=float('inf')
        release=api.release_details(dict(self.summary(),track_count=2))
        self.assertEqual(len(calls),1);self.assertTrue(release['tracks_loaded'])
        result=coverage([dict(album='Local album',title='One',duration=180)],dict(releases=[release]))[0]
        self.assertEqual(result['state'],'Owned partial');self.assertEqual(result['missing'][0]['id'],'12')

    def test_whole_release_export_requires_approval_and_uses_album_url(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'app.sqlite3');store.enqueue(self.summary())
            self.assertEqual(store.export('txt'),'')
            with store.connect() as db: db.execute('UPDATE queue SET approved=1')
            self.assertEqual(store.export('txt'),'https://tidal.com/browse/album/1\n')

    def test_pacer_spaces_requests_and_remains_cancellable(self):
        current=[0.0]
        with patch('library_manager.tidal.time.monotonic',lambda:current[0]), patch('library_manager.tidal.time.sleep',lambda delay:current.__setitem__(0,current[0]+delay)):
            pacer=RequestPacer(.5);pacer.wait(lambda:False);pacer.wait(lambda:False)
            self.assertGreaterEqual(current[0],.5)
            from library_manager.tidal import CatalogueError
            with self.assertRaises(CatalogueError): pacer.wait(lambda:True)
