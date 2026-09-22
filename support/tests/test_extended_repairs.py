import copy,tempfile,time,unittest
from pathlib import Path
from unittest.mock import Mock
from mutagen.flac import FLAC
from library_manager.core import Store,scan
from library_manager.maintenance import audio_digest
from library_manager.extended_review import discover,discovery_for,recording_match,repair_preview,apply_repairs
from library_manager.linking import saved_links
from test_maintenance import fixture

class ExtendedRepairTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=(Path(self.temp.name)/'music').resolve();self.path=self.root/'Wrong'/'Album'/'song.flac';fixture(self.path)
        audio=FLAC(self.path);audio.update(albumartist=['Wrong'],title=['Song'],album=['Old Album'],isrc=['GB-TEST'],bpm=['124'],initialkey=['8A'],genre=['House']);audio.save()
        self.store=Store(Path(self.temp.name)/'db');scan(self.store,self.root)
        self.local=self.store.tracks()
        self.release=dict(id='100',title='Right Album',date='2024-03-04',album_artists=['Main Artist','Second Artist'],album_artist_ids=['10','20'],disc_count=2,tracks_loaded=True,tag_checked_at=time.time(),credits_complete=True,
            tracks=[dict(id='101',isrc='GBTEST',title='Song',track_number=3,disc_number=2,duration=1)])
    def test_cached_discovery_repair_and_database_roundtrip(self):
        self.store.save_preferences('tag-review:GB:100',self.release)
        api=Mock();found=discover(self.local,self.store,'GB',api)
        self.assertEqual(len(found),1);self.assertEqual(api.mock_calls,[])
        before=self.path.read_bytes();digest=audio_digest(self.path)
        plan=repair_preview(self.store,found[0]);self.assertEqual(self.path.read_bytes(),before)
        self.assertIn('/Disc 02/',plan[0]['target'])
        self.assertEqual(apply_repairs(self.store,'GB',plan),1)
        target=Path(plan[0]['target']);self.assertTrue(target.exists());self.assertFalse(self.path.exists())
        tags=FLAC(target)
        self.assertEqual(tags['albumartist'],['Main Artist, Second Artist'])
        self.assertEqual(tags['date'],['2024-03-04'])
        self.assertEqual(tags['tracknumber'],['3']);self.assertEqual(tags['discnumber'],['2'])
        self.assertEqual(tags['bpm'],['124']);self.assertEqual(tags['initialkey'],['8A']);self.assertEqual(tags['genre'],['House'])
        self.assertEqual(audio_digest(target),digest)
        self.assertEqual(saved_links(self.store,'GB')[str(target)][1]['ids'],{'album_id':'100','track_id':'101'})
        self.assertEqual({m['tidal_id'] for m in self.store.linked_mappings()},{'10','20'})
        self.assertNotIn('Wrong',self.store.artists())
    def test_stale_preview_and_collision_keep_original(self):
        found=discovery_for(self.release,self.local);plan=repair_preview(self.store,found)
        target=Path(plan[0]['target']);target.parent.mkdir(parents=True);target.write_bytes(b'existing')
        with self.assertRaises(ValueError):repair_preview(self.store,found)
        target.unlink()
        audio=FLAC(self.path);audio['comment']=['External edit'];audio.save()
        with self.assertRaises(ValueError):apply_repairs(self.store,'GB',plan)
        self.assertTrue(self.path.exists());self.assertFalse(target.exists())
    def test_conflicting_isrc_and_unverified_title_rejected(self):
        self.assertFalse(recording_match(dict(isrc='A',title='Song',duration=180),dict(isrc='B',title='Song',duration=180)))
        self.assertFalse(recording_match(dict(title='Song'),dict(title='Song')))
        self.assertFalse(recording_match(dict(title='Song',duration=180),dict(title='Song (Remix)',duration=180)))
        self.assertTrue(recording_match(dict(title='Song',duration=180),dict(title='Song',duration=181)))
    def test_url_search_uses_album_details_once_and_reuses_cache(self):
        api=Mock();api.album_tag_details.return_value=self.release
        found=discover(self.local,self.store,'GB',api,query='https://tidal.com/album/100')
        self.assertEqual(found[0]['release_id'],'100');api.album_tag_details.assert_called_once()
        api.reset_mock();again=discover(self.local,self.store,'GB',api,query='100')
        self.assertEqual(again[0]['release_id'],'100');self.assertEqual(api.mock_calls,[])
    def test_same_recording_in_multiple_releases_is_user_choice(self):
        second=dict(self.release,id='200',title='Other Edition')
        self.store.save_preferences('tag-review:GB:100',self.release);self.store.save_preferences('tag-review:GB:200',second)
        before=self.path.read_bytes()
        found=discover(self.local,self.store,'GB',Mock())
        self.assertEqual({d['release_id'] for d in found},{'100','200'})
        self.assertEqual(self.path.read_bytes(),before)
