import copy,time,json,subprocess,sys,tempfile,unittest
from pathlib import Path
from unittest.mock import Mock,patch
from mutagen.flac import FLAC
from library_manager.core import Store,scan,read_metadata
from library_manager.maintenance import inspect_file,apply_one,replan,first
from library_manager.organisation import layout_path
from library_manager.library_workflows import workflow_plan,validate_operation,inspect_snapshot
from library_manager.tag_review import check_album_tags
from library_manager.dj_metadata import DJMetadata
from library_manager.tidal import RequestPacer
from test_maintenance import fixture


class LibraryWorkflows(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.base=Path(self.temp.name).resolve();self.root=self.base/'music'
        self.path=self.root/'Wrong'/'Album'/'song.flac';fixture(self.path)
        self.store=Store(self.base/'db')

    def test_all_album_artist_values_lead_scanning_and_paths(self):
        audio=FLAC(self.path);audio['albumartist']=['Asketa & Natan Chaim','Mo Falk'];audio.save()
        row=inspect_file(self.path,self.root)
        expected='Asketa & Natan Chaim, Mo Falk'
        self.assertEqual(first(row['tags'],'albumartist'),expected)
        self.assertEqual(read_metadata(self.path)['artist'],expected)
        self.assertEqual(layout_path(self.root,row['tags']).relative_to(self.root).parts[0],expected)
        del audio['albumartist'];audio['artist']=['Wrong'];audio.save()
        row=replan([inspect_file(self.path,self.root)])[0]
        self.assertNotIn('albumartist',row['changes'],'Folders must never generate artist tags')

    def test_legacy_first_artist_cache_is_refreshed_once(self):
        audio=FLAC(self.path);audio['albumartist']=['One','Two'];audio.save()
        scan(self.store,self.root)
        with self.store.connect() as db:
            old=read_metadata(self.path);old['artist']='One';old.pop('artist_grouping_version')
            db.execute('UPDATE local_files SET metadata=?',(json.dumps(old),))
        self.assertEqual(scan(self.store,self.root)['read'],1)
        self.assertEqual(set(self.store.artists()),{'One, Two'})
        self.assertEqual(scan(self.store,self.root)['read'],0)

    def test_each_operation_is_independent_and_pending_tags_do_not_drive_moves(self):
        row=inspect_file(self.path,self.root)
        row.update(overrides={'albumartist':['Correct']},metadata_changes={'bpm':['123']},artwork_proposal={'path':'prepared','sha256':'example'})
        snapshot=[row]
        tags=workflow_plan(snapshot,'tags')[0];self.assertEqual(tags['target'],tags['path']);self.assertNotIn('bpm',tags['changes']);self.assertNotIn('artwork_change',tags)
        metadata=workflow_plan([tags],'metadata')[0];self.assertEqual(metadata['changes'],{'bpm':['123']});self.assertEqual(metadata['target'],metadata['path'])
        artwork=workflow_plan([metadata],'artwork')[0];self.assertEqual(artwork['changes'],{});self.assertIn('artwork_change',artwork)
        move=workflow_plan([artwork],'organise')[0];self.assertEqual(move['changes'],{});self.assertNotIn('artwork_change',move);self.assertIn('/Wrong/',move['target']);self.assertNotIn('/Correct/',move['target'])
        for mode,plan in [('tags',tags),('metadata',metadata),('artwork',artwork),('organise',move)]:validate_operation(plan,mode)
        move['changes']['albumartist']=['Injected']
        with self.assertRaises(ValueError):validate_operation(move,'organise')
        self.assertEqual(workflow_plan([artwork],'tags')[0]['changes']['albumartist'],['Correct'])

    def test_apply_tags_then_moves_uses_saved_artist_preserving_dj_analysis(self):
        audio=FLAC(self.path);audio['bpm']=['121.000000'];audio['initialkey']=['Abm'];audio['serato_beatgrid']=['analysis-data'];audio.save()
        row=inspect_file(self.path,self.root);row['overrides']={'albumartist':['Mo Falk']}
        tags=workflow_plan([row],'tags')[0];validate_operation(tags,'tags');apply_one(tags,self.store)
        self.assertTrue(self.path.exists());self.assertEqual(FLAC(self.path)['albumartist'],['Mo Falk'])
        move=workflow_plan([inspect_file(self.path,self.root)],'organise')[0]
        before=self.path.read_bytes();validate_operation(move,'organise');apply_one(move,self.store)
        self.assertEqual(Path(move['target']).read_bytes(),before)
        self.assertEqual(self.store.tracks()[0]['artist'],'Mo Falk')

    def test_incremental_inspection_reuses_unchanged_and_discards_stale_proposals(self):
        other=self.root/'Wrong/Album/other.flac';fixture(other)
        first=inspect_snapshot(self.root);first[0]['overrides']={'albumartist':['Pending']}
        other.unlink();audio=FLAC(self.path);audio['albumartist']=['External fix'];audio.save()
        second=inspect_snapshot(self.root,first)
        self.assertEqual(len(second),1);self.assertEqual(second[0]['tags']['albumartist'],['External fix']);self.assertNotIn('overrides',second[0])
        with patch('library_manager.library_workflows.inspect_file',side_effect=AssertionError('Unnecessary tag reread')):
            self.assertEqual(inspect_snapshot(self.root,second),second)

    def test_verified_catalogue_corrections_record_ids_without_replacing_dj_tags(self):
        row=inspect_file(self.path,self.root);row['tags'].update(tracknumber=['1/1'],isrc=['GBTEST'],bpm=['121.000000'],initialkey=['Abm'])
        track=dict(id='11',isrc='GBTEST',title='Correct title',artists=['Artist','Guest'],track_number=1,disc_number=1,bpm=140,key='C',key_scale='MAJOR')
        release=dict(id='1',artist='Mo Falk',title='Refraction (Remixes)',date='2024-05-01',tracks=[track],tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,album_artists=['Mo Falk'],available=True)
        self.store.save_catalogue('1','GB',dict(releases=[release]))
        api=Mock();api.track_tag_details.return_value=track;api.recording_releases.return_value=[]
        checked=check_album_tags([row],self.store,'GB',api,correct_metadata=True)
        plan=workflow_plan(checked,'links')[0]
        self.assertEqual(plan['changes']['tidal_track_id'],['11']);self.assertEqual(plan['changes']['tidal_album_id'],['1'])
        self.assertEqual(plan['changes']['title'],['Correct title']);self.assertEqual(plan['changes']['albumartist'],['Mo Falk'])
        self.assertNotIn('bpm',plan['changes']);self.assertNotIn('initialkey',plan['changes'])
        # A stored release ID resolves otherwise ambiguous identical editions.
        duplicate=dict(release,id='2',album_artists=['Other'])
        self.store.save_catalogue('1','GB',dict(releases=[release,duplicate]))
        row['tags']['tidal_album_id']=['1']
        resolved=check_album_tags([row],self.store,'GB',api,correct_metadata=True)[0]
        self.assertEqual(resolved['catalogue_choice']['id'],'1')
        release['credits_complete']=False;self.store.save_catalogue('1','GB',dict(releases=[release]))
        self.assertFalse(check_album_tags([row],self.store,'GB',api,correct_metadata=True)[0]['catalogue_options'])

    def test_python_tidal_metadata_worker_is_reused_cached_and_recording_checked(self):
        (self.base/'downloader-session').mkdir()
        code="import json,sys;print(json.dumps({'event':'ready'}),flush=True)\nfor line in sys.stdin:\n r=json.loads(line);print(json.dumps({'event':'metadata','track':{'id':r['id'],'isrc':'GBTEST','bpm':125,'key':'CSharp','key_scale':'MINOR'}}),flush=True)"
        factory=Mock(side_effect=lambda command,**kw:subprocess.Popen([sys.executable,'-u','-c',code],**kw))
        with DJMetadata(self.store,pacer=RequestPacer(0),process_factory=factory) as dj:
            value=dj.lookup({'id':'11','isrc':'GBTEST'});self.assertEqual(value['bpm'],125)
            self.assertEqual(dj.lookup({'id':'12','isrc':'GBTEST'})['key'],'CSharp');self.assertEqual(factory.call_count,1)
        with DJMetadata(self.store,process_factory=Mock(side_effect=AssertionError('Cached value should not start runtime'))) as dj:
            self.assertEqual(dj.lookup({'id':'11','isrc':'GBTEST'}),value)
        with self.assertRaises(ValueError):DJMetadata.verified({'id':'11','isrc':'GBTEST'},{'id':'11','isrc':'WRONG'})

    def test_python_tidal_metadata_worker_handles_unavailable_tracks_without_stopping(self):
        (self.base/'downloader-session').mkdir()
        code="import json,sys;print(json.dumps({'event':'ready'}),flush=True)\nfor line in sys.stdin:\n r=json.loads(line)\n if r['id']=='99':print(json.dumps({'event':'track_unavailable','id':'99','message':'Not found'}),flush=True)\n else:print(json.dumps({'event':'metadata','track':{'id':r['id'],'isrc':'GBTEST','bpm':125,'key':'CSharp','key_scale':'MINOR'}}),flush=True)"
        factory=Mock(side_effect=lambda command,**kw:subprocess.Popen([sys.executable,'-u','-c',code],**kw))
        with DJMetadata(self.store,pacer=RequestPacer(0),process_factory=factory) as dj:
            self.assertEqual(dj.lookup({'id':'99','isrc':'GBTEST'}),{})
            self.assertFalse(dj.disabled)
            self.assertEqual(dj.lookup({'id':'11','isrc':'GBTEST'})['bpm'],125)
            self.assertFalse(dj.disabled)
            self.assertEqual(dj.lookup({'id':'99','isrc':'GBTEST'}),{})

    def test_extended_metadata_cache_is_partitioned_by_market(self):
        (self.base/'downloader-session').mkdir()
        code="import json,sys;print(json.dumps({'event':'ready','market':'GB'}),flush=True)\nfor line in sys.stdin:\n r=json.loads(line);print(json.dumps({'event':'metadata','track':{'id':r['id'],'isrc':'GBTEST','bpm':125}}),flush=True)"
        factory=Mock(side_effect=lambda command,**kw:subprocess.Popen([sys.executable,'-u','-c',code],**kw))
        with DJMetadata(self.store,pacer=RequestPacer(0),process_factory=factory,market='GB') as dj:
            self.assertEqual(dj.lookup({'id':'11','isrc':'GBTEST'})['bpm'],125)
        with DJMetadata(self.store,pacer=RequestPacer(0),process_factory=factory,market='US') as dj:
            self.assertEqual(dj.lookup({'id':'11','isrc':'GBTEST'})['bpm'],125)
        self.assertEqual(factory.call_count,2)

    def test_python_tidal_fallback_fills_release_gaps_and_preserves_dj_values(self):
        row=inspect_file(self.path,self.root);row['tags']['isrc']=['GBTEST'];row['tags']['tracknumber']=['1/1']
        track=dict(id='11',isrc='GBTEST',title='Incandescent',track_number=1,disc_number=1)
        release=dict(id='1',artist='Mo Falk',title='Refraction (Remixes)',date='2024-05-01',tracks=[track],tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,album_artists=['Mo Falk'],available=True)
        self.store.save_catalogue('1','GB',dict(releases=[release]));api=Mock();api.track_tag_details.return_value=track;api.recording_releases.return_value=[]
        fallback=Mock(return_value=dict(bpm=125,key='CSharp',key_scale='MINOR'))
        result=check_album_tags([row],self.store,'GB',api,enrich=True,dj_lookup=fallback)[0]
        self.assertEqual(result['metadata_changes']['initialkey'],['12A']);self.assertEqual(result['metadata_changes']['bpm'],['125'])
        row['tags'].update(bpm=['121.000000'],key=['8A']);fallback.reset_mock()
        fallback.return_value=dict(bpm=140,key='D',key_scale='MAJOR',release=dict(id='1',barcode='001234567890',type='EP',copyright='Explicit copyright'))
        filled=check_album_tags([row],self.store,'GB',api,enrich=True,dj_lookup=fallback)[0]['metadata_changes']
        self.assertEqual(fallback.call_args.kwargs['album_id'],'1')
        self.assertEqual(filled['upc'],['001234567890']);self.assertEqual(filled['releasetype'],['ep'])
        self.assertNotIn('bpm',filled);self.assertNotIn('initialkey',filled)
        self.assertNotIn('label',filled,'Copyright must not be used as label')

    def test_superseded_singles_in_workflows_and_apply(self):
        from library_manager.library_workflows import has_changes
        # Create a single track and an EP release track with the same title and artist
        single_path = self.root/'Artist'/'Throat Song - Single'/'01 - Throat Song.flac'
        ep_path = self.root/'Artist'/'The EP'/'02 - Throat Song.flac'
        ep_other = self.root/'Artist'/'The EP'/'01 - Intro.flac'
        for p in (single_path, ep_path, ep_other): fixture(p)
        
        a = FLAC(single_path); a['isrc']=['TESTMATCH']; a['title']=['Throat Song']; a['artist']=['Dylan Brady']; a['album']=['Throat Song']; a['tracknumber']=['1']; a.save()
        b = FLAC(ep_path); b['isrc']=['TESTMATCH']; b['title']=['Throat Song']; b['artist']=['Dylan Brady']; b['album']=['Peace & Love']; b['tracknumber']=['2']; b['tracktotal']=['4']; b.save()
        c = FLAC(ep_other); c['title']=['Intro']; c['artist']=['Dylan Brady']; c['album']=['Peace & Love']; c['tracknumber']=['1']; c['tracktotal']=['4']; c.save()

        scan(self.store, self.root)
        snapshot = inspect_snapshot(self.root)
        plan_tags = workflow_plan(snapshot, 'tags')
        single_row_tags = next(r for r in plan_tags if r['path'] == str(single_path))
        self.assertIsNone(single_row_tags.get('superseded_by'))
        self.assertFalse(any('Superseded single' in iss for iss in single_row_tags['issues']))

        for mode in ('metadata','artwork','links','summary'):
            self.assertTrue(all(not r.get('superseded_by') for r in workflow_plan(snapshot, mode)))
        plan_org = workflow_plan(snapshot, 'organise', superseded_singles=True)
        single_row = next(r for r in plan_org if r['path'] == str(single_path))
        self.assertTrue(single_row.get('superseded_by'))
        self.assertTrue(has_changes(single_row))
        self.assertTrue(any('Superseded single' in iss for iss in single_row['issues']))
        validate_operation(single_row, 'organise')
        
        # Test applying moves it to trash and updates store
        with patch('library_manager.maintenance.trash_file', side_effect=lambda p: p.rename(p.with_suffix('.test-trash'))):
            res = apply_one(single_row, self.store)
        self.assertEqual(res, 'applied')
        self.assertFalse(single_path.exists())
        self.assertTrue(ep_path.exists())
        self.assertEqual(self.store.rows('SELECT present FROM local_files WHERE path=?', (str(single_path),))[0]['present'], 0)

    def test_tag_issues_populated_in_workflow_plan(self):
        audio = FLAC(self.path)
        audio['date'] = ['2020/05/12']
        audio['discnumber'] = ['1']
        audio['key'] = ['Am']
        audio['lyrics'] = ['Some lyrics']
        audio.save()

        snapshot = inspect_snapshot(self.root)
        # Check that with all cleanup options OFF, issues are still reported in row['issues']
        plan_off = workflow_plan(snapshot, 'tags', dates=False, discs=False, keys=False, remove_lyrics=False)
        row_off = plan_off[0]
        self.assertIn('Date formatting unstandardised', row_off['issues'])
        self.assertIn('Track/disc numbers unstandardised', row_off['issues'])
        self.assertIn('Musical key not Camelot', row_off['issues'])
        self.assertIn('Contains lyrics tags', row_off['issues'])
        self.assertEqual(row_off['changes'], {})

        # Check that with cleanup options ON, changes are populated
        plan_on = workflow_plan(snapshot, 'tags', dates=True, discs=True, keys=True, remove_lyrics=True)
        row_on = plan_on[0]
        self.assertEqual(row_on['changes']['date'], ['2020-05-12'])
        self.assertEqual(row_on['changes']['discnumber'], ['01'])
        self.assertEqual(row_on['changes']['initialkey'], ['8A'])
        self.assertEqual(row_on['changes']['lyrics'], [])


    def test_persisted_inspection_reuses_tags_but_detects_external_changes(self):
        from library_manager.freshness import prepare_library,cached_inspection
        first=prepare_library(self.store,self.root,[],lambda:False,lambda _:None)
        with patch('library_manager.library_workflows.inspect_file',side_effect=AssertionError('Unchanged FLAC reread')):
            second=prepare_library(self.store,self.root,[],lambda:False,lambda _:None)
        self.assertEqual(first[0]['tags'],second[0]['tags'])
        audio=FLAC(self.path);audio['title']=['External edit'];audio.save()
        third=prepare_library(self.store,self.root,[],lambda:False,lambda _:None)
        self.assertEqual(third[0]['tags']['title'],['External edit'])
        self.path.unlink()
        self.assertEqual(prepare_library(self.store,self.root,[],lambda:False,lambda _:None),[])
        self.assertEqual(cached_inspection(self.store,self.root),[])
