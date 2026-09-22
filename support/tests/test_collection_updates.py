import time
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock
from library_manager.core import Store,scan
from library_manager.connections import check_connections
from library_manager.tag_review import check_album_tags
from library_manager.maintenance import replan
from library_manager.tidal import Tidal,CatalogueError

class CollectionUpdates(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);self.store=Store(self.root/'db')
    def test_compilations_retained_in_inventory_but_not_artist_matching(self):
        music=self.root/'music';music.mkdir();(music/'a.flac').touch()
        scan(self.store,music,lambda p:dict(artist='Various Artists',album='Compilation'))
        self.store.mapping('Various Artists','1','confirmed','manual',True)
        self.assertEqual(len(self.store.tracks()),1);self.assertEqual(self.store.artists(),{})
        self.assertEqual(len(self.store.artists(True)),1);self.assertEqual(self.store.linked_mappings(),[])
    def test_removed_counts_once_and_move_retag_refreshes_artist(self):
        music=self.root/'music';music.mkdir();p=music/'a.flac';p.write_text('one')
        scan(self.store,music,lambda p:dict(artist='Old',album='A'))
        p.rename(music/'b.flac')
        r=scan(self.store,music,lambda p:dict(artist='New',album='A'))
        self.assertEqual(r['missing'],1);self.assertEqual(set(self.store.artists()),{'New'})
        self.assertEqual(scan(self.store,music)['missing'],0)
        (music/'b.flac').unlink();self.assertEqual(scan(self.store,music)['missing'],1)
        self.assertEqual(self.store.tracks(),[]);self.assertEqual(scan(self.store,music)['missing'],0)
    def test_connection_checks_use_one_collection_page_and_no_browser(self):
        credentials=Mock();account=Mock();account.load.return_value={'saved':True}
        api=Mock();api.BASE='https://openapi.tidal.com/v2/';api.url.return_value='search'
        result=check_connections(credentials,account,api)
        self.assertEqual(api.get.call_count,2);account.connect.assert_not_called()
        self.assertIn('Catalogue search: connected',result)
        api.get.side_effect=CatalogueError('denied',status=403)
        self.assertIn('Favourites access: denied',check_connections(credentials,account,api))
    def row(self):
        return dict(path=str(self.root/'Wrong'/'Single'/'a.flac'),root=str(self.root),blocked='',has_artwork=True,
                    tags=dict(albumartist=['Wrong'],artist=['Wrong, Guest'],album=['Album'],title=['Song'],date=['2020-01-01'],isrc=['GB123'],tracknumber=['1']))
    def release(self,ident='1',artist='Right'):
        return dict(id=ident,artist=artist,title='Album',date='2020-01-01',tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),
                    album_artists=[artist],available=True,tracks=[dict(id='10',title='Song',isrc='GB123',track_number=1,disc_number=1)])
    def test_recording_verified_repair_reuses_cache_and_preserves_track_credit(self):
        self.store.save_catalogue('1','GB',dict(releases=[self.release()]))
        api=Mock();rows=check_album_tags([self.row()],self.store,'GB',api)
        api.album_tag_details.assert_not_called()
        plan=replan(rows);self.assertEqual(plan[0]['changes']['albumartist'],['Right'])
        self.assertNotIn('artist',plan[0]['changes'])
        self.assertNotIn('albumartist',replan(rows,repair=False)[0]['changes'])
    def test_conflicting_editions_and_wrong_recording_do_not_propose(self):
        self.store.save_catalogue('1','GB',dict(releases=[self.release(),self.release('2','Other')]))
        rows=check_album_tags([self.row()],self.store,'GB',Mock())
        self.assertNotIn('catalogue_choice',rows[0]);self.assertEqual(len(rows[0]['catalogue_options']),2)
        row=self.row();row['tags']['isrc']=['NOPE']
        rows=check_album_tags([row],self.store,'GB',Mock());self.assertFalse(rows[0]['catalogue_options'])
    def test_album_change_requires_numbering(self):
        r=self.release(artist='Wrong');r['title']='Remixes';r['tracks'][0]['disc_number']=None
        self.store.save_catalogue('1','GB',dict(releases=[r]))
        rows=check_album_tags([self.row()],self.store,'GB',Mock());self.assertNotIn('catalogue_choice',rows[0])
        r['tracks'][0]['disc_number']=1;self.store.save_catalogue('1','GB',dict(releases=[r]))
        rows=check_album_tags([self.row()],self.store,'GB',Mock())
        self.assertEqual(rows[0]['catalogue_choice']['track_id'],'10');self.assertEqual(rows[0]['catalogue_choice']['changes']['discnumber'],['01'])
    def test_fallback_recording_lookup_and_joint_credits_require_choice(self):
        api = Mock();api.recording_releases.return_value = [self.release()]
        detailed = self.release();detailed['album_artists'] = ['Right','Guest']
        api.album_tag_details.return_value = detailed
        rows = check_album_tags([self.row()],self.store,'GB',api)
        # A fully checked fixture is already usable without another network call.
        self.assertTrue(api.recording_releases.called)
        self.store.save_catalogue('1','GB',dict(releases=[detailed]))
        rows = check_album_tags([self.row()],self.store,'GB',api)
        self.assertNotIn('catalogue_choice',rows[0])
        self.assertEqual(len(rows[0]['catalogue_options']),3)

    def test_sibling_folder_cohesion_auto_selects_album(self):
        self.store.save_catalogue('1','GB',dict(releases=[self.release('1','Main'),self.release('2','Alternate')]))
        # Simulate sibling track in the same directory already linked to release 2
        import json
        with self.store.connect() as db:
            db.execute("INSERT OR REPLACE INTO track_links VALUES(?,?,?,?)",
                       ('/music/Artist/Album/02 - Other.flac', 'GB', json.dumps([1,1]), json.dumps({'ids': {'album_id': '2', 'track_id': '20'}})))
        row = self.row()
        row['path'] = '/music/Artist/Album/01 - Song.flac'
        rows = check_album_tags([row], self.store, 'GB', Mock())
        self.assertIn('catalogue_choice', rows[0])
        self.assertEqual(rows[0]['catalogue_choice']['id'], '2')
        self.assertIn('matched sibling tracks in folder', rows[0]['catalogue_note'])

    def test_replan_layout_target_preservation(self):
        from library_manager.maintenance import replan
        row = self.row()
        row['root'] = '/music'
        row['path'] = '/music/Wrong/Wrong/01 - Song.flac'
        row['tags']['albumartist'] = ['Artist']
        row['layout'] = {'template': '{albumartist}/{album}/{tracknumber} - {title}'}
        planned = replan([row], organise=False)
        self.assertEqual(planned[0]['target'], '/music/Wrong/Wrong/01 - Song.flac')
        self.assertEqual(planned[0]['layout_target'], '/music/Artist/Album/01 - Song.flac')
        self.assertIn('Folder or filename differs from tag layout', planned[0]['issues'])

        planned_org = replan([row], organise=True)
        self.assertEqual(planned_org[0]['target'], '/music/Artist/Album/01 - Song.flac')
        self.assertEqual(planned_org[0]['layout_target'], '/music/Artist/Album/01 - Song.flac')

    def test_coverage_includes_local_tracks(self):
        from library_manager.core import coverage
        release = dict(id='100', title='Album', available=True, tracks=[dict(id='t1', title='Song 1', duration=180, isrc='GB001')])
        tracks = [dict(album='Album', title='Song 1', duration=180, path='/music/Artist/Album/01 - Song.flac', isrc='GB001')]
        results = coverage(tracks, dict(releases=[release]))
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0]['state'], 'Owned complete')
        self.assertEqual(results[0]['local_tracks'], ['/music/Artist/Album/01 - Song.flac'])

    def test_detect_superseded_singles_separate_and_same_folder(self):
        from library_manager.maintenance import detect_superseded_singles
        # Lost In Pacific pattern (separate folders: single vs album)
        single = {
            'path': '/music/Lost In Pacific/Follow The Sun (Single)/01 - Follow The Sun.flac',
            'duration': 210,
            'tags': {'albumartist': ['Lost In Pacific'], 'album': ['Follow The Sun - Single'], 'title': ['Follow The Sun'], 'tracknumber': ['1'], 'isrc': ['LIP01']}
        }
        album_t1 = {
            'path': '/music/Lost In Pacific/Follow The Sun/01 - Talk To.flac',
            'duration': 195,
            'tags': {'albumartist': ['Lost In Pacific'], 'album': ['Follow The Sun'], 'title': ['Talk To'], 'tracknumber': ['1']}
        }
        album_t2 = {
            'path': '/music/Lost In Pacific/Follow The Sun/02 - Follow The Sun.flac',
            'duration': 210,
            'tags': {'albumartist': ['Lost In Pacific'], 'album': ['Follow The Sun'], 'title': ['Follow The Sun'], 'tracknumber': ['2'], 'isrc': ['LIP01']}
        }
        snapshot = [single, album_t1, album_t2]
        for row in snapshot:row['tags']['artist']=['Lost In Pacific']
        sup = detect_superseded_singles(snapshot)
        self.assertIn(single['path'], sup)
        self.assertEqual(sup[single['path']]['album_track'], '2')
        self.assertIn('Follow The Sun', sup[single['path']]['album_title'])

        # Dylan Brady pattern (same folder: multi-track EP with single track 1 and album track > 1)
        dylan_single = {
            'path': '/music/Dylan Brady/Needle Guy/01 - Needle Guy.flac',
            'duration': 140,
            'tags': {'albumartist': ['Dylan Brady'], 'album': ['Needle Guy'], 'title': ['Needle Guy'], 'tracknumber': ['1']}
        }
        dylan_t1 = {
            'path': '/music/Dylan Brady/Needle Guy/01 - Throat Song.flac',
            'duration': 160,
            'tags': {'albumartist': ['Dylan Brady'], 'album': ['Needle Guy'], 'title': ['Throat Song'], 'tracknumber': ['1']}
        }
        dylan_t4 = {
            'path': '/music/Dylan Brady/Needle Guy/04 - Needle Guy.flac',
            'duration': 140,
            'tags': {'albumartist': ['Dylan Brady'], 'album': ['Needle Guy'], 'title': ['Needle Guy'], 'tracknumber': ['4']}
        }
        snapshot_dylan = [dylan_single, dylan_t1, dylan_t4]
        for row in snapshot_dylan:row['tags']['artist']=['Dylan Brady']
        sup_dylan = detect_superseded_singles(snapshot_dylan)
        self.assertNotIn(dylan_single['path'], sup_dylan)
        dylan_single['tags']['isrc']=['DYLAN1']
        dylan_t4['tags']['isrc']=['DYLAN1']
        sup_dylan=detect_superseded_singles(snapshot_dylan)
        self.assertIn(dylan_single['path'], sup_dylan)
        self.assertEqual(sup_dylan[dylan_single['path']]['album_track'], '4')

    def test_store_ignored_local_files(self):
        self.assertEqual(self.store.ignored_local_files(), set())
        self.store.ignore_local_file('/music/track1.flac')
        self.assertEqual(self.store.ignored_local_files(), {'/music/track1.flac'})
        self.store.ignore_local_file('/music/track2.flac')
        self.assertEqual(self.store.ignored_local_files(), {'/music/track1.flac', '/music/track2.flac'})
        self.store.unignore_local_file('/music/track1.flac')
        self.assertEqual(self.store.ignored_local_files(), {'/music/track2.flac'})

if __name__=='__main__':unittest.main()
