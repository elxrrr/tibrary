import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import Mock
from mutagen.flac import FLAC
from library_manager.core import Store,scan
from library_manager.maintenance import inspect_file,apply_one
from library_manager.library_workflows import workflow_plan,validate_operation
from library_manager.linking import link_recordings,saved_links,attach_links,save_result
from library_manager.organisation import layout_path
from library_manager.view_data import build_view
from test_maintenance import fixture

class BackgroundLinking(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name).resolve()/'music';self.path=self.root/'Wrong/Album/song.flac';fixture(self.path)
        audio=FLAC(self.path);audio['isrc']=['GBTEST'];audio['tracknumber']=['1/1'];audio.save()
        self.store=Store(Path(self.temp.name)/'db');scan(self.store,self.root)
        self.row=inspect_file(self.path,self.root)
        self.track=dict(id='11',isrc='GBTEST',title='Correct title',track_number=1,disc_number=1)
        self.release=dict(id='1',artist='Wrong',title='Album',date='2024-05-01',tracks=[self.track],tracks_loaded=True,
                          tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,album_artists=['Wrong'],available=True)
        self.store.save_catalogue('1','GB',dict(id='1',releases=[self.release]))
        self.store.mapping('Wrong','1','confirmed','Test',True)
        self.api=Mock();self.api.track_tag_details.return_value=self.track;self.api.recording_releases.return_value=[]
        self.dj=Mock(return_value=dict(bpm=125,key='CSharp',key_scale='MINOR'))

    def test_local_cleanup_never_uses_online_proposals_and_can_move(self):
        row=self.row
        row['tags'].update(date=['2024/05/01'],discnumber=['01/02'],lyrics=['Remove'])
        row['catalogue_choice']={'changes':{'albumartist':['Online artist'],'tidal_track_id':['99']}}
        plan=workflow_plan([row],'tags',remove_lyrics=True,organise_local=True)[0]
        self.assertEqual(plan['changes'],dict(date=['2024-05-01'],discnumber=['01'],disctotal=['02'],tracknumber=['01'],tracktotal=['01'],lyrics=[]))
        self.assertIn('/Wrong/Refraction (Remixes) (2024)/Disc 01/01.01 - ',plan['target'])
        validate_operation(plan,'tags')
        self.assertNotIn('TIDAL', ' '.join(plan['issues']))
        single=layout_path(self.root,dict(row['tags'],discnumber=['1']),year='2024')
        self.assertNotIn('/Disc ',str(single));self.assertTrue(single.name.startswith('01 - '))

    def test_persist_resume_and_existing_dj_tags_still_checked(self):
        self.row['tags'].update(bpm=['121'],key=['8A'])
        before=self.path.read_bytes()
        link_recordings([self.row],self.store,'GB',self.api,self.dj)
        payload=saved_links(self.store,'GB')[str(self.path)][1]
        self.assertEqual(payload['ids'],dict(album_id='1',track_id='11'))
        self.assertTrue(payload['dj_complete']);self.dj.assert_called_once()
        self.assertNotIn('bpm',payload['metadata_changes']);self.assertNotIn('initialkey',payload['metadata_changes'])
        self.assertEqual(payload['dj_checks'][0]['bpm'],['125'])
        self.assertEqual(self.path.read_bytes(),before)
        api=Mock(side_effect=AssertionError('Resume should use saved links'))
        link_recordings([self.row],Store(self.store.path),'GB',api,api)
        self.assertEqual(api.mock_calls,[])
        self.assertEqual(attach_links([self.row],self.store,'GB')[0]['linked_ids']['track_id'],'11')
        data=build_view(self.store,'GB')
        self.assertEqual(data['artists']['Wrong'][0]['tidal_track_id'],'11')

    def test_stale_response_is_rejected_and_new_tags_invalidate_link(self):
        link_recordings([self.row],self.store,'GB',self.api,self.dj)
        old=attach_links([self.row],self.store,'GB')[0]
        audio=FLAC(self.path);audio['albumartist']=['Edited'];audio.save();scan(self.store,self.root)
        self.assertFalse(save_result(self.store,'GB',old))
        fresh=attach_links([inspect_file(self.path,self.root)],self.store,'GB')[0]
        self.assertNotIn('linked_ids',fresh)
        self.assertNotEqual(build_view(self.store,'GB')['artists']['Edited'][0].get('tidal_track_id'),'11')

    def test_manual_placement_survives_a_late_background_result(self):
        link_recordings([self.row],self.store,'GB',self.api,self.dj)
        row=attach_links([self.row],self.store,'GB')[0]
        row['catalogue_choice']['artist']='My reviewed grouping'
        self.assertTrue(save_result(self.store,'GB',row,manual=True))
        row['catalogue_choice']['artist']='Automatic grouping'
        self.assertTrue(save_result(self.store,'GB',row))
        self.assertEqual(saved_links(self.store,'GB')[str(self.path)][1]['catalogue_choice']['artist'],'My reviewed grouping')
        row['catalogue_choice']['id']='other'
        self.assertFalse(save_result(self.store,'GB',row))

    def test_incomplete_dj_check_retries_and_unavailable_is_not_a_success(self):
        self.dj.return_value={}
        link_recordings([self.row],self.store,'GB',self.api,self.dj)
        payload=saved_links(self.store,'GB')[str(self.path)][1]
        self.assertFalse(payload['dj_complete']);self.assertIn('incomplete',payload['note'])
        self.dj.return_value=dict(bpm=None,key=None,key_scale=None)
        link_recordings([self.row],self.store,'GB',self.api,self.dj)
        self.assertEqual(self.dj.call_count,2)
        payload=saved_links(self.store,'GB')[str(self.path)][1]
        self.assertTrue(payload['dj_complete']);self.assertIn('not supplied',payload['note'])
