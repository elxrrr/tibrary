import time
import unittest
import tempfile
from pathlib import Path
from library_manager.core import Store
from library_manager.discovery import checked_recently, needs_verification, working_releases, verify_releases
from library_manager.tidal import Tidal, CatalogueError


def item(ident):
    return dict(local_artists=['A'],release=dict(id=ident,title='Album',date='2020-01-01',type='ALBUM',track_count=2,explicit=False,available=True))

class ReleaseLinkTests(unittest.TestCase):
    def test_expiry_and_manual_only_keep_unknown_links_unverified(self):
        row=item('saved');release=row['release']
        release['link_checked_at']=time.time()-10*86400
        self.assertTrue(checked_recently(release))
        self.assertFalse(needs_verification([row]))
        self.assertFalse(checked_recently(release,7))
        release['link_checked_at']=time.time()-31*86400
        self.assertTrue(needs_verification([row]))
        self.assertTrue(checked_recently(release,0))
        self.assertEqual(working_releases([row],max_age_days=0),[row])
        for timestamp in (0,None,time.time()+86400):
            release['link_checked_at']=timestamp
            self.assertFalse(checked_recently(release,0))
        release.update(link_checked_at=time.time(),available=None)
        self.assertFalse(checked_recently(release,0))

    def test_bounded_batches_continue_duplicates_and_newest_first(self):
        older=item('older');older['release']['date']='2019-01-01'
        rows=[older]+[item(str(n)) for n in range(102)];calls=[]
        class Api:
            def release_availability(self,r,force=False):
                calls.append(r['id'])
                return dict(r,available=r['id'] in ('101','older'),link_checked_at=time.time())
        def check():verify_releases(rows,Api(),lambda i,r:None,lambda:False,lambda _:None,max_requests=100)
        check();self.assertEqual(calls,[str(n) for n in range(100)])
        check();self.assertEqual(calls,[str(n) for n in range(102)]+['older'])
        check();self.assertEqual(len(calls),103)
        self.assertEqual(len(working_releases(rows)),2)

    def test_refresh_and_restart_preserve_checks_in_same_market(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'cache.db';store=Store(path)
            saved=dict(item('1')['release'],available=False,link_checked_at=time.time())
            store.save_catalogue('artist','GB',dict(releases=[saved]))
            refreshed=dict(item('1')['release'],title='Updated title')
            store.save_catalogue('artist','GB',dict(releases=[refreshed]))
            store=Store(path)
            release=store.cache('artist','GB')['releases'][0]
            self.assertTrue(checked_recently(release))
            self.assertFalse(release['available'])
            self.assertEqual(release['title'],'Updated title')
            self.assertNotIn('link_checked_at',refreshed)
            store.save_catalogue('artist','US',dict(releases=[refreshed]))
            self.assertFalse(checked_recently(store.cache('artist','US')['releases'][0]))
            newer=dict(refreshed,link_checked_at=saved['link_checked_at']+1)
            store.save_catalogue('artist','GB',dict(releases=[newer]))
            self.assertTrue(store.cache('artist','GB')['releases'][0]['available'])

    def test_dead_duplicate_replaced_and_checks_cached(self):
        rows=[item('dead'),item('working')];calls=[]
        class Api:
            def release_availability(self,r,force=False):
                calls.append(r['id']);return dict(r,available=r['id']=='working',link_checked_at=time.time())
        save=lambda i,r:i.update(release=r)
        self.assertEqual(working_releases(rows),[])
        verify_releases(rows,Api(),save,lambda:False,lambda message:None)
        self.assertEqual([i['release']['id'] for i in working_releases(rows)],['working'])
        verify_releases(rows,Api(),save,lambda:False,lambda message:None)
        self.assertEqual(calls,['dead','working'])
        different=item('deluxe');different['release'].update(title='Album (Deluxe)',link_checked_at=time.time())
        self.assertEqual(len(working_releases(rows+[different])),2)

    def test_only_definitive_missing_status_is_cached(self):
        api=Tidal('GB')
        for status in (404,410,401,429,500):
            def fail(*args,**kwargs):raise CatalogueError('Failure',status=status)
            api.get=fail
            if status in (404,410):
                self.assertFalse(api.release_availability(item('x')['release'],force=True)['available'])
            else:
                with self.assertRaises(CatalogueError):api.release_availability(item('x')['release'],force=True)
