import tempfile,time,unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock,patch
from mutagen.flac import FLAC
from library_manager.organisation import safe_component,layout_path
from library_manager.musical_keys import key_changes
from library_manager.download_policy import validate_stream,validate_file
from library_manager.linking import link_recordings,attach_links,retain_after_apply,saved_links
from library_manager.library_workflows import workflow_plan
from library_manager.maintenance import inspect_file,apply_one
from library_manager.core import Store,scan
from test_maintenance import fixture

class ConsistentLibrary(unittest.TestCase):
    def test_portable_punctuation_and_devices(self):
        cases={'Science/Visions':'Science - Visions','If You Can\'t Sleep / Drift Away':"If You Can't Sleep - Drift Away",'album1: a lot of remixes':'album1 - a lot of remixes','This is where it ends...':'This is where it ends','Haunt // Bed':'Haunt - Bed','Belong - Mirth':'Belong - Mirth','CON':'_CON','aux.txt':'_aux.txt','LPT1':'_LPT1','Music??':'Music','Odysée':'Odysée'}
        for platform in ('darwin','win32'):
            with patch('library_manager.organisation.sys.platform',platform):
                for source,expected in cases.items():self.assertEqual(safe_component(source),expected)
        target=layout_path('/Music',dict(albumartist=['A'],album=['B'],title=['End...'],tracknumber=['3'],discnumber=['2']), '2024',True)
        self.assertEqual(str(target),'/Music/A/B (2024)/Disc 02/02.03 - End.flac')

    def test_key_notation_preserves_pitch_mode_and_conflicts(self):
        self.assertEqual(key_changes({'tkey':['8A'],'initialkey':['A minor']})[0],{'initialkey':['8A']})
        self.assertEqual(key_changes({'key':['G major']})[0],{'initialkey':['9B']})
        self.assertEqual(key_changes({'key':['D'],'initialkey':['Dm']})[0],{})
        self.assertTrue(key_changes({'key':['unknown']})[1])

    def test_download_requires_lossless_flac_and_cd_resolution(self):
        manifest=SimpleNamespace(codecs='FLAC');stream=SimpleNamespace(audio_quality='LOSSLESS',bit_depth=16,sample_rate=44100)
        validate_stream(manifest,stream)
        for field,value in [('audio_quality','HI_RES'),('audio_quality','HIGH'),('bit_depth',24),('sample_rate',48000)]:
            other=SimpleNamespace(**vars(stream));setattr(other,field,value)
            with self.assertRaises(ValueError):validate_stream(manifest,other)
        with self.assertRaises(ValueError):validate_stream(SimpleNamespace(codecs='MQA'),stream)

    def test_download_file_headers_are_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'track.flac';fixture(path)
            with self.assertRaises(ValueError):validate_file(path)  # The fixture is 8 kHz.
            audio=FLAC(path);audio.info.sample_rate=44100;audio.save();validate_file(path)
            audio=FLAC(path);audio.info.bits_per_sample=24;audio.save()
            with self.assertRaises(ValueError):validate_file(path)

    def test_apply_metadata_and_move_retain_central_links_without_api(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()/'music';path=root/'Wrong/Album/song.flac';fixture(path)
            audio=FLAC(path);audio['isrc']=['GBTEST'];audio['tracknumber']=['1/1'];audio.save()
            store=Store(Path(directory)/'db');scan(store,root);row=inspect_file(path,root)
            track=dict(id='11',isrc='GBTEST',title='Incandescent',track_number=1,disc_number=1)
            release=dict(id='1',artist='Wrong',title='Refraction (Remixes)',date='2024-05-01',tracks=[track],tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,album_artists=['Wrong'],available=True)
            store.save_catalogue('1','GB',dict(id='1',releases=[release]));api=Mock();api.track_tag_details.return_value=track
            dj=Mock(return_value=dict(bpm=125,key='C',key_scale='MAJOR'))
            link_recordings([row],store,'GB',api,dj)
            row=attach_links([row],store,'GB')[0]
            plan=workflow_plan([row],'metadata')[0];apply_one(plan,store)
            fresh=attach_links([inspect_file(path,root)],store,'GB')[0]
            self.assertEqual(fresh['linked_ids']['track_id'],'11')
            move=workflow_plan([fresh],'organise')[0];apply_one(move,store)
            moved=inspect_file(move['target'],root);retain_after_apply(store,'GB',move,moved)
            linked=attach_links([moved],store,'GB')[0]
            self.assertEqual(linked['linked_ids'],dict(track_id='11',album_id='1'))
            self.assertEqual(linked['metadata_changes'],{})
            api.reset_mock();dj.reset_mock();link_recordings([moved],store,'GB',api,dj)
            self.assertEqual(api.mock_calls,[]);dj.assert_not_called()
            self.assertTrue(saved_links(store,'GB')[moved['path']][1]['dj_complete'])
