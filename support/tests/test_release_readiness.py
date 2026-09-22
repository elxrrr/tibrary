import time
import copy
import hashlib
import json
import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock,patch
from mutagen.flac import FLAC,Picture
from PySide6.QtGui import QImage,QColor
from PySide6.QtCore import QBuffer,QIODevice
from library_manager.core import Store
from library_manager.enrichment import missing_tags
from library_manager.tag_review import check_album_tags
from library_manager.maintenance import inspect_file,replan,apply_one,audio_digest
from library_manager.artwork import normalise_cover,checked_picture,prepare_cover,prepare_existing_cover
from library_manager.tidal import CatalogueError
from library_manager import backends
from library_manager.download_bridge import organise_download
from test_maintenance import fixture


def cover(size):
    image=QImage(size,size,QImage.Format.Format_RGB32);image.fill(QColor('blue'))
    buffer=QBuffer();buffer.open(QIODevice.OpenModeFlag.WriteOnly);assert image.save(buffer,'JPEG')
    return bytes(buffer.data())

class ReleaseReadinessTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.base=Path(self.temp.name).resolve();self.store=Store(self.base/'db')
        self.release=dict(id='1',title='Album',artist='Artist',date='2020-01-01',album_artists=['Artist'],genres=['Electronic'],copyright={'text':'Copyright Owner'},barcode='01234',disc_count=1,available=True,tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3)
        self.track=dict(id='11',title='Music??',isrc='GBTEST',bpm=125,key='CSharp',key_scale='MINOR',track_number=1,disc_number=1,genres=['House'])
        self.release['tracks']=[self.track]
    def test_existing_dj_tags_are_never_replaced_or_duplicated(self):
        for tags in [dict(key=['8A'],bpm=['127.000']),dict(initialkey=['Am'],tempo=['120'])]:
            changes=missing_tags(tags,self.release,self.track)
            self.assertNotIn('bpm',changes);self.assertNotIn('initialkey',changes)
            self.assertNotIn('label',changes);self.assertEqual(changes['genre'],['House'])
        changes=missing_tags({},self.release,self.track)
        self.assertEqual(changes['bpm'],['125']);self.assertEqual(changes['initialkey'],['12A'])
        self.track['key_scale']='UNKNOWN';self.assertNotIn('initialkey',missing_tags({},self.release,self.track))
        for value in ('unknown','nan',None):
            self.track['bpm']=value;self.assertNotIn('bpm',missing_tags({},self.release,self.track))
    def test_enrichment_verifies_recording_and_reuses_track_metadata(self):
        row=dict(root=str(self.base),path=str(self.base/'Artist/Album/Music.flac'),blocked='',tags=dict(albumartist=['Artist'],album=['Album'],title=['Music??'],date=['2020-01-01'],isrc=['GBTEST'],key=['8A'],bpm=['127']))
        self.store.save_catalogue('1','GB',dict(releases=[self.release]))
        api=Mock();api.track_tag_details.return_value=dict(self.track,artists=['Artist']);api.recording_releases.return_value=[]
        rows=check_album_tags([row,row],self.store,'GB',api,enrich=True)
        self.assertEqual(api.track_tag_details.call_count,1)
        self.assertEqual(rows[0]['metadata_changes']['genre'],['House'])
        self.assertNotIn('bpm',rows[0]['metadata_changes']);self.assertNotIn('initialkey',rows[0]['metadata_changes'])
        row['tags']['isrc']=['WRONG'];rows=check_album_tags([row],self.store,'GB',api,enrich=True)
        self.assertEqual(rows[0]['metadata_changes'],{})
    def test_cover_resizes_but_never_upscales_and_repair_preserves_dj_audio(self):
        with self.assertRaises(ValueError):normalise_cover(cover(640))
        data=normalise_cover(cover(1600));image=QImage.fromData(data)
        self.assertEqual((image.width(),image.height()),(1280,1280))
        path=self.base/'Artist/Album/song.flac';fixture(path)
        audio=FLAC(path);audio['key']=['8A'];audio['bpm']=['127.000'];audio.save()
        digest=audio_digest(path);row=inspect_file(path,self.base)
        cache=self.base/'cover.jpg';cache.write_bytes(data)
        proposal=dict(path=str(cache),sha256=hashlib.sha256(data).hexdigest(),width=1280,height=1280)
        row['artwork_change']=proposal;row=replan([row],repair=False,dates=False,discs=False)[0]
        self.assertEqual(apply_one(row,self.store),'applied')
        audio=FLAC(path);front=[p for p in audio.pictures if p.type==3]
        self.assertEqual((front[0].width,front[0].height),(1280,1280));self.assertEqual(audio['key'],['8A']);self.assertEqual(audio['bpm'],['127.000'])
        self.assertEqual(audio_digest(path),digest)
        cache.write_bytes(b'bad')
        with self.assertRaises(ValueError):checked_picture(proposal)
    def test_cover_toggle_uses_existing_preview(self):
        path=self.base/'Artist/Album/song.flac';fixture(path);row=inspect_file(path,self.base)
        row['artwork_change']={'path':'test','sha256':'test'};row['covers_enabled']=False
        with patch('library_manager.maintenance.FLAC',side_effect=AssertionError('reread')):
            off=replan([row]);self.assertNotIn('artwork_change',off[0]);off[0]['covers_enabled']=True
            self.assertIn('artwork_change',replan(off)[0])
    def test_cover_header_dimensions_and_existing_large_artwork(self):
        path=self.base/'Artist/Album/song.flac';fixture(path)
        audio=FLAC(path);picture=Picture();picture.type=3;picture.mime='image/jpeg';picture.data=cover(1600)
        audio.add_picture(picture);audio.save()  # Tidaler does not write picture width/height.
        row=inspect_file(path,self.base);self.assertEqual(row['cover_size'],(1600,1600))
        proposal=prepare_existing_cover(row,self.store);self.assertEqual(checked_picture(proposal).width,1280)
        api=Mock();api.recording_releases.return_value=[]
        checked=check_album_tags([row],self.store,'GB',api,enrich=True,covers=True)[0]
        self.assertIn('artwork_change',checked)
    def test_remote_cover_is_cached_and_untrusted_sources_rejected(self):
        release=dict(cover_files=[dict(href='https://resources.tidal.com/test.jpg',meta=dict(width=1280,height=1280))])
        transport=Mock(side_effect=lambda *a,**kw:io.BytesIO(cover(1280)))
        first=prepare_cover(release,self.store,transport=transport)
        self.assertEqual(first,prepare_cover(release,self.store,transport=transport));self.assertEqual(transport.call_count,1)
        self.assertNotIn('Authorization',transport.call_args.args[0].headers)
        release['cover_files'][0]['href']='https://example.com/image.jpg'
        with self.assertRaises(CatalogueError):prepare_cover(release,self.store,transport=transport)
    def test_download_uses_same_tag_layout_and_keeps_key(self):
        stage=self.base/'stage';path=stage/'source.flac';fixture(path)
        audio=FLAC(path);audio['title']=['Music??'];audio['bpm']=['120'];audio['key']=['A'];audio.save()
        target=organise_download(path,stage,{},'11','1',{'discnumber':['1'],'disctotal':['1']})
        self.assertEqual(target.name,'01 - Music.flac');self.assertFalse(path.exists())
        audio=FLAC(target);self.assertEqual(audio['key'],['A']);self.assertEqual(audio['tidal_track_id'],['11'])
    def test_failed_backend_build_never_changes_active_installation(self):
        resources=self.base/'resources';home=resources/'backends';home.mkdir(parents=True)
        active=home/'active.json';active.write_text('{"build":"old","commits":{}}')
        def failed(command,cancel):raise ValueError('simulated install failure')
        with self.assertRaises(ValueError):backends.update(resources=resources,runner=failed)
        self.assertEqual(json.loads(active.read_text())['build'],'old');self.assertEqual(list(home.glob('build-*')),[])
    def test_backend_activation_and_rollback(self):
        resources=self.base/'resources';baseline=resources/'tidaler/.venv/bin/python';baseline.parent.mkdir(parents=True);baseline.touch()
        def runner(command,cancel):
            if command[0]=='git' and 'rev-parse' in command:return 'a'*40
            if '-m' in command and 'venv' in command:
                python=Path(command[-1])/'bin/python';python.parent.mkdir(parents=True);python.touch()
            return ''
        backends.update(resources=resources,runner=runner)
        self.assertIn('backends',str(backends.active_runtime(resources)))
        backends.rollback(resources);self.assertEqual(backends.active_runtime(resources),baseline)
if __name__=='__main__':unittest.main()
