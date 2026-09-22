import time
import copy
import errno
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock,patch
from mutagen.flac import FLAC
from library_manager.core import Store,scan
from library_manager.maintenance import inspect_file,apply_one,replan
from library_manager.freshness import prepare_library
from library_manager.tag_review import check_album_tags
from library_manager.view_data import build_view,filter_coverage
from test_maintenance import fixture


class RefreshAndAssociations(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name).resolve()/'music';self.path=self.root/'Wrong/Album/track.flac';fixture(self.path)
        self.store=Store(Path(self.temp.name)/'db')

    def test_external_edits_refresh_tags_index_and_drop_stale_proposals(self):
        scan(self.store,self.root);old=inspect_file(self.path,self.root);old['metadata_changes']={'label':['Stale']}
        audio=FLAC(self.path);audio['albumartist']=['HEALTH'];audio['tracknumber']=['1/10'];audio.save()
        fresh=prepare_library(self.store,self.root,[old],lambda:False,lambda _:None)
        self.assertEqual(fresh[0]['tags']['albumartist'],['HEALTH']);self.assertTrue(fresh[0]['needs_artist_match'])
        self.assertNotIn('metadata_changes',fresh[0]);self.assertEqual(set(self.store.artists()),{'HEALTH'})
        with patch('library_manager.library_workflows.inspect_file',side_effect=AssertionError('Unchanged FLAC reread')):
            prepare_library(self.store,self.root,fresh,lambda:False,lambda _:None)
        self.path.unlink();self.assertEqual(prepare_library(self.store,self.root,fresh,lambda:False,lambda _:None),[])
        self.assertEqual(self.store.artists(),{})

    def test_same_volume_organisation_never_copies_or_hashes_audio(self):
        row=replan([inspect_file(self.path,self.root)],repair=False,dates=False,discs=False,organise=True)[0]
        before=self.path.read_bytes();inode=self.path.stat().st_ino
        with patch('library_manager.maintenance.shutil.copy2',side_effect=AssertionError('Unexpected copy')),patch('library_manager.maintenance.audio_digest',side_effect=AssertionError('Unexpected hash')):
            self.assertEqual(apply_one(row,self.store),'applied')
        target=Path(row['target']);self.assertEqual(target.read_bytes(),before);self.assertEqual(target.stat().st_ino,inode)
        self.assertFalse(self.path.exists());self.assertEqual(self.store.tracks()[0]['path'],str(target))

    def test_move_falls_back_safely_when_drive_does_not_support_hard_links(self):
        row=replan([inspect_file(self.path,self.root)],repair=False,dates=False,discs=False,organise=True)[0]
        before=self.path.read_bytes();real_link=os.link;calls=[]
        def link(source,target):
            calls.append(source)
            if len(calls)==1:raise OSError(errno.EOPNOTSUPP,'Unsupported hard link')
            return real_link(source,target)
        with patch('library_manager.maintenance.os.link',side_effect=link):self.assertEqual(apply_one(row,self.store),'applied')
        self.assertEqual(Path(row['target']).read_bytes(),before);self.assertFalse(self.path.exists())

    def release(self,ident,title,tracks):
        return dict(id=ident,title=title,artist='HEALTH',album_artists=['HEALTH'],date='2024-05-01',tracks=tracks,tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,available=True,type='ALBUM')

    def test_cached_title_without_recording_does_not_block_direct_isrc_lookup(self):
        track=dict(id='123',isrc='GBRIGHT',title='Incandescent',artists=['HEALTH'],track_number=1,disc_number=1)
        wrong=self.release('1','Refraction (Remixes)',[dict(track,id='900',isrc='WRONG')]);right=self.release('2','Refraction (Remixes)',[track])
        self.store.save_catalogue('health','GB',dict(releases=[wrong]))
        row=inspect_file(self.path,self.root);row['tags']['isrc']=['GBRIGHT'];row['tags']['tracknumber']=['1/1']
        api=Mock();api.recording_releases.return_value=[right];api.track_tag_details.return_value=track
        result=check_album_tags([row],self.store,'GB',api,correct_metadata=True)[0]
        api.recording_releases.assert_called_once_with('GBRIGHT')
        self.assertEqual(result['catalogue_choice']['id'],'2')
        self.assertEqual(result['catalogue_choice']['changes']['tidal_track_id'],['123'])

    def test_current_album_tags_replace_stale_saved_edition_before_python_tidal(self):
        track=dict(id='123',isrc='GBRIGHT',title='Incandescent',artists=['HEALTH'],track_number=1,disc_number=1)
        wrong=self.release('1','Old single',[track]);right=self.release('2','Refraction (Remixes)',[track])
        self.store.save_catalogue('health','GB',dict(releases=[wrong]))
        row=inspect_file(self.path,self.root);row['tags'].update(tracknumber=['1/1'],isrc=['GBRIGHT'],tidal_album_id=['1'],tidal_track_id=['123'])
        api=Mock();api.recording_releases.return_value=[right];api.track_tag_details.return_value=track
        fallback=Mock(return_value=dict(release=dict(id='2',barcode='00123')))
        result=check_album_tags([row],self.store,'GB',api,enrich=True,dj_lookup=fallback)[0]
        self.assertEqual(fallback.call_args.kwargs['album_id'],'2')
        self.assertEqual(result['metadata_changes']['upc'],['00123'])
        # Enrichment preserves existing IDs: correcting a stale link requires the reviewed tag correction.
        self.assertNotIn('tidal_album_id',result['metadata_changes'])

    def test_bulk_view_keeps_multiple_id_coverage_and_local_compilation_counts(self):
        scan(self.store,self.root)
        self.store.mapping('Wrong','a','confirmed','chosen',manual=True,extra_ids=['b'])
        for ident,title in [('a','Refraction (Remixes)'),('b','New album')]:
            release=self.release(ident,title,[]);release.update(tracks_loaded=False,track_count=10,explicit=False,link_checked_at=10**10)
            self.store.save_catalogue(ident,'GB',dict(id=ident,releases=[release]))
        data=build_view(self.store,'GB')
        self.assertEqual(data['track_count'],1)
        self.assertEqual({i['state'] for i in data['compared']},{'Present locally','Missing release'})
        candidates,_,_=filter_coverage(data,{},('All missing releases','','All statuses','All types'),True)
        self.assertEqual([i['release']['id'] for i in candidates],['b'])
