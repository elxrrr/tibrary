import tempfile,time,unittest,copy
from pathlib import Path
from unittest.mock import patch
from library_manager.core import Store
from library_manager.cached_tidal import CachedTidal
from library_manager.tidal import Tidal

class SharedCacheTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.store=Store(Path(self.tmp.name)/'db')
        self.api=CachedTidal('GB',store=self.store)
        self.release=dict(id='1',track_count=2,title='Album')
        self.details=dict(self.release,tracks_loaded=True,item_count=2,tracks=[dict(id='11',track_number=1),dict(id='12',track_number=2)])
    def test_cross_instance_cache_is_detached_and_market_scoped(self):
        with patch.object(Tidal,'release_details',return_value=self.details) as fetch:
            first=self.api.release_details(self.release);first['tracks'].clear()
            second=CachedTidal('GB',store=self.store).release_details(self.release)
            self.assertEqual(len(second['tracks']),2);self.assertEqual(fetch.call_count,1)
            CachedTidal('US',store=self.store).release_details(self.release)
            self.assertEqual(fetch.call_count,2)
    def test_changed_summary_expiry_and_force_refresh(self):
        with patch.object(Tidal,'release_details',return_value=self.details) as fetch:
            self.api.release_details(self.release)
            self.api.release_details(dict(self.release,track_count=3))
            self.assertEqual(fetch.call_count,2)
            self.api.release_details(self.release,force=True)
            self.assertEqual(fetch.call_count,3)
            saved=self.store.preferences('tag-review:GB:1');saved['details_checked_at']=time.time()-86400*31
            self.store.save_preferences('tag-review:GB:1',saved)
            self.api.release_details(self.release);self.assertEqual(fetch.call_count,4)
    def test_failure_preserves_cache_and_details_do_not_refresh_credit_age(self):
        saved=dict(self.details,tag_checked_at=1,tag_credits_checked=True,metadata_schema=3)
        self.store.save_preferences('tag-review:GB:1',saved)
        with patch.object(Tidal,'release_details',side_effect=RuntimeError('offline')):
            with self.assertRaises(RuntimeError):self.api.release_details(self.release)
        self.assertEqual(self.store.preferences('tag-review:GB:1'),saved)
        with patch.object(Tidal,'release_details',return_value=self.details):self.api.release_details(self.release)
        self.assertEqual(self.store.preferences('tag-review:GB:1')['tag_checked_at'],1)
    def test_album_and_track_metadata_reused(self):
        with patch.object(Tidal,'album_tag_details',return_value=dict(self.details,tag_credits_checked=True)),patch.object(Tidal,'track_tag_details',return_value={'id':'11','bpm':125,'key':'C'}) as track:
            self.api.album_tag_details(self.release)
            self.api.track_tag_details({'id':'11'})
            other=CachedTidal('GB',store=self.store)
            self.assertEqual(other.track_tag_details({'id':'11'})['bpm'],125)
            self.assertEqual(track.call_count,1)
        with patch.object(Tidal,'album_tag_details',side_effect=AssertionError('cached')):
            self.assertTrue(other.album_tag_details(self.release)['tag_credits_checked'])
