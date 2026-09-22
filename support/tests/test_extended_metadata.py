import datetime
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace as Item
from unittest.mock import Mock
from library_manager.enrichment import missing_tags,needs_extended_metadata
from library_manager.dj_metadata import DJMetadata
from library_manager.metadata_bridge import read_metadata
from library_manager.library_workflows import validate_operation,workflow_plan,LYRIC_TAGS
from library_manager.maintenance import inspect_file,apply_one,audio_digest
from library_manager.core import Store
from mutagen.flac import FLAC
from test_maintenance import fixture


class ExtendedMetadata(unittest.TestCase):
    def test_verified_edition_is_requested_once_without_lyrics_or_media(self):
        artist=Item(name='Artist')
        track=Item(id=11,isrc='GBTEST',bpm=120,key='C',key_scale='MAJOR',full_name='Song',artists=[artist],copyright='Track copyright',album=Item(id=999,upc='WRONG'))
        album=Item(id=22,name='Selected edition',artists=[artist],release_date=datetime.datetime(2024,1,2),upc='001234567890',copyright='Album copyright',type='EP',num_volumes=2)
        session=Mock(spec=['track','album']);session.track.return_value=track;session.album.return_value=album
        cache={}
        first=read_metadata(session,'11','22',cache,pause=lambda _:None)
        read_metadata(session,'12','22',cache,pause=lambda _:None)
        session.album.assert_called_once_with('22')
        verified=DJMetadata.verified({'id':'11','isrc':'GBTEST'},first,'22')
        self.assertEqual(verified['release']['barcode'],'001234567890')
        self.assertEqual(verified['release']['date'],'2024-01-02')
        self.assertEqual(verified['isrc'],'GBTEST')
        self.assertNotIn('lyrics',verified)
        with self.assertRaises(ValueError):DJMetadata.verified({'id':'11'},first,'999')

    def test_explicit_tags_and_aliases_no_lyrics_or_invented_label(self):
        release=dict(id='22',barcode='001234567890',type='EP',label={'name':'Real Label'},copyright='Owner',date='2024-01-02')
        track=dict(id='11',credits=[dict(role='Composer',name='Writer'),dict(role='Producer',name='Producer'),dict(role='Engineer',name='Engineer')],lyrics='Must not be added')
        tags=missing_tags({},release,track)
        self.assertEqual(tags['label'],['Real Label']);self.assertEqual(tags['upc'],['001234567890'])
        self.assertEqual(tags['composer'],['Writer']);self.assertEqual(tags['engineer'],['Engineer'])
        self.assertEqual(tags['url'],['https://tidal.com/track/11']);self.assertNotIn('lyrics',tags)
        existing={'barcode':['Different existing barcode'],'recordlabel':['Existing label'],'lyrics':['Existing lyrics']}
        filled=missing_tags(existing,release,track)
        self.assertNotIn('upc',filled);self.assertNotIn('label',filled);self.assertNotIn('lyrics',filled)
        self.assertNotIn('label',missing_tags({},dict(release,label=None),track))
        for key in ('upc','label','lyrics'):
            row=dict(path='same',target='same',operation='metadata',tags=existing,changes={key:['New']})
            with self.assertRaises(ValueError):validate_operation(row,'metadata')

    def test_complete_metadata_needs_no_fallback(self):
        release=dict(id='22',barcode='00123',type='ALBUM',date='2024-01-01',title='Album',album_artists=['Artist'],disc_count=1)
        track=dict(id='11',isrc='GBTEST',bpm=120,key='C',key_scale='MAJOR',title='Track',artists=['Artist'],copyright='Owner')
        self.assertFalse(needs_extended_metadata({},release,track))
        release.pop('barcode')
        self.assertTrue(needs_extended_metadata({},release,track))
        self.assertFalse(needs_extended_metadata({'barcode':['Existing']},release,track))

    def test_optional_lyrics_removal_preserves_audio_and_all_other_metadata(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();path=root/'song.flac';fixture(path);store=Store(root/'db')
            audio=FLAC(path)
            for key in LYRIC_TAGS:audio[key]=['Existing lyrics']
            audio['bpm']=['121.000000'];audio['initialkey']=['Abm'];audio['serato_beatgrid']=['Custom data'];audio.save()
            original=dict(FLAC(path).tags);digest=audio_digest(path);pictures=[p.write() for p in audio.pictures]
            snapshot=[inspect_file(path,root)]
            default=workflow_plan(snapshot,'tags',dates=False,discs=False,keys=False)[0]
            self.assertFalse(default['changes'])
            selected=workflow_plan(snapshot,'tags',dates=False,discs=False,remove_lyrics=True,keys=False)[0]
            self.assertEqual(selected['changes'],{key:[] for key in LYRIC_TAGS})
            self.assertEqual(selected['target'],str(path));validate_operation(selected,'tags')
            for operation in ('metadata','artwork','organise'):
                plan=workflow_plan([selected],operation,remove_lyrics=True)[0]
                self.assertFalse(any(key in plan['changes'] for key in LYRIC_TAGS))
            self.assertFalse(workflow_plan([selected],'tags',dates=False,discs=False,remove_lyrics=False,keys=False)[0]['changes'])
            self.assertEqual(apply_one(selected,store),'applied')
            checked=FLAC(path)
            self.assertEqual(dict(checked.tags),{k:v for k,v in original.items() if k not in LYRIC_TAGS})
            self.assertEqual(audio_digest(path),digest);self.assertEqual([p.write() for p in checked.pictures],pictures)
