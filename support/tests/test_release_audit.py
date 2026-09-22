"""Release regressions: identities, safe matching, failures and local-only counts."""
import copy
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch
from library_manager.core import Store
from library_manager.favourites_view import favourite_rows
from library_manager.view_data import normalize_copyright
from library_manager.musical_keys import supplied_key
from library_manager.tag_review import check_album_tags
from library_manager.tidal import CatalogueError
from library_manager.linking import link_recordings

class ReleaseAuditTests(unittest.TestCase):
    def test_artist_identity_union_and_counts(self):
        favourites=[{'id':1,'name':'Artist'}, {'id':'2','name':'ARTIST'}, {'id':'3','name':'Alias'}, {'id':'4','name':'Absent'}]
        local={'Artist':[{},{}], 'Local only':[{}]}
        mappings=[{'artist':'Artist','tidal_id':str(i)} for i in (1,2,3)]
        rows=favourite_rows(favourites,local,mappings)
        self.assertEqual(len(rows),3)
        artist=next(r for r in rows if r[0]=='Artist')
        self.assertEqual(artist[1:4],('In library','2','1, 2, 3'))
        self.assertEqual(sum(r[1]=='Missing locally' for r in rows),1)
        self.assertEqual(sum(r[1]=='Local only' for r in rows),1)
        self.assertEqual(favourite_rows([],{},[]),[])

    def test_copyright_keeps_company_letters(self):
        self.assertEqual(normalize_copyright('℗ 2025 Capitol Records'),'capitol records')
        self.assertEqual(normalize_copyright('(C) 2024 Foreign Family Collective'),'foreign family collective')

    def test_key_forms(self):
        self.assertEqual(supplied_key({'key':'8A','key_scale':'MINOR'}),'8A')
        self.assertEqual(supplied_key({'musical_key':'C# Minor'}),'12A')
        self.assertEqual(supplied_key({'key':'G','key_scale':'MAJOR'}),'9B')
        self.assertIsNone(supplied_key({'initialkey':'8A','tkey':'9B'}))

    def test_discovery_failure_is_reported_without_unbound_error(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'db');api=Mock()
            api.recording_releases.side_effect=CatalogueError('Recording unavailable',status=404)
            rows=[dict(path=str(Path(directory)/'one.flac'), tags={'isrc':['TEST'],'title':['One']})]
            results=check_album_tags(rows,store,'GB',api)
            self.assertIn('Recording unavailable',results[0]['catalogue_note'])

    def test_many_editions_reuse_each_fetched_detail_without_aborting(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'db');api=Mock()
            api.recording_releases.return_value=[dict(id=str(i),title='Other') for i in range(12)]
            rows=[dict(path=str(Path(directory)/str(i)),tags={'isrc':['TEST'],'title':[str(i)]}) for i in range(2)]
            api.album_tag_details.side_effect=lambda r:dict(r,tracks=[],tracks_loaded=True,album_artists=['Artist'])
            completed=[]
            check_album_tags(rows,store,'GB',api,on_result=completed.append)
            self.assertEqual(len(completed),2)
            self.assertEqual(api.album_tag_details.call_count,12)
            self.assertTrue(all('Too many possible editions' not in r['catalogue_note'] for r in completed))

    def test_fatal_linking_failure_is_not_a_finished_job(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'db')
            row=dict(path=str(Path(directory)/'song.flac'),tags={})
            with patch('library_manager.linking.check_album_tags',side_effect=CatalogueError('Unauthorized',status=401)):
                with self.assertRaises(CatalogueError): link_recordings([row],store,'GB',Mock())

    def test_sibling_number_never_overrides_conflicting_isrc(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'db');api=Mock()
            api.recording_releases.return_value=[]
            api.album_tag_details.return_value=dict(id='1',title='Album',album_artists=['Artist'],tracks=[dict(id='11',title='Different song',isrc='OTHER',track_number=1,disc_number=1,duration=180)])
            base=dict(path=str(Path(directory)/'a.flac'),tags={'album':['Album'],'albumartist':['Artist'],'tidal_album_id':['1'],'title':['First'],'isrc':['OTHER']},duration=180)
            second=dict(path=str(Path(directory)/'b.flac'),tags={'album':['Album'],'albumartist':['Artist'],'title':['Second'],'isrc':['CONFLICT'],'tracknumber':['1']},duration=180)
            results=check_album_tags([base,second],store,'GB',api)
            self.assertFalse(results[1].get('catalogue_choice'))
            self.assertFalse(results[1].get('catalogue_options'))

    def test_trash_failure_keeps_original(self):
        from library_manager.maintenance import trash_file
        import sys
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'track.flac';path.write_bytes(b'original')
            with patch.dict(sys.modules, {'send2trash':Mock(send2trash=Mock(side_effect=OSError('Unavailable')))}), patch('library_manager.maintenance.subprocess.run',return_value=Mock(returncode=1)):
                with self.assertRaises(ValueError):trash_file(path)
            self.assertEqual(path.read_bytes(),b'original')

    def test_grouped_sort_keeps_tracks_and_identity(self):
        from library_manager.virtual_table import RowsModel
        from PySide6.QtCore import Qt
        model=RowsModel(['Title'],None)
        model.replace_custom([['B'],['B track'],['A'],['A track']],['b','bt','a','at'],{0:'release',1:'track',2:'release',3:'track'},{(1,0):'B tooltip'})
        model.sort(0,Qt.SortOrder.AscendingOrder)
        self.assertEqual(model.keys,['a','at','b','bt'])
        self.assertEqual(model.tooltips[(3,0)],'B tooltip')
        self.assertEqual(model.rowCount(),4)

    def test_search_reports_authorization_and_rate_limit_errors(self):
        from library_manager.tidal import Tidal
        for method in ('search_albums','search_tracks'):
            for status in (401,429):
                api=Tidal();api.get=Mock(side_effect=CatalogueError('Failed',status=status))
                with self.assertRaises(CatalogueError):getattr(api,method)('Song')

    def test_recording_album_fetch_does_not_hide_rate_limit(self):
        from library_manager.tidal import Tidal
        api=Tidal();api.entities=Mock(return_value=[{'relationships':{'albums':{'data':[{'id':'1'}]}}}])
        api.get=Mock(side_effect=CatalogueError('Rate limited',status=429))
        with self.assertRaises(CatalogueError):api.recording_releases(isrc='TEST')
