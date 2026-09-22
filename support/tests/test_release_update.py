import copy,io,json,tempfile,time,unittest,unicodedata
from pathlib import Path
from unittest.mock import Mock,patch
import numpy as np
import soundfile as sf
from mutagen.flac import FLAC,Picture
from PIL import Image
from library_manager.core import Store,scan,coverage
from library_manager.release_matching import structure,recording_matches
from library_manager.tag_review import check_album_tags
from library_manager.download_bridge import organise_download,publish_files
from library_manager.linking import save_result,saved_links,pending_paths,state_signature,invalidate_related_links,attach_links
from library_manager.maintenance import inspect_file
from library_manager.mqa_audit import scan_samples,audit_file,audit_library,PATTERN
from library_manager.optimizations import find_optimizations,validate_consolidation,consolidate
from library_manager.recommendations import recommend
from library_manager.view_data import filter_coverage
from library_manager.downloads import index_downloaded_files


def audio_file(path,number=1,total=1,title='Song',isrc='TEST1',album='Single',album_id='',disc=1,discs=1):
    path.parent.mkdir(parents=True,exist_ok=True)
    sf.write(path,np.zeros((44100,2)),44100,subtype='PCM_16')
    audio=FLAC(path);audio.update(albumartist=['Artist'],artist=['Artist','Guest'],album=[album],title=[title],
                                tracknumber=[str(number)],tracktotal=[str(total)],discnumber=[str(disc)],disctotal=[str(discs)],
                                date=['2020-01-01'],isrc=[isrc],bpm=['120'],initialkey=['8A'])
    if album_id:audio.update(tidal_album_id=[album_id],tidal_track_id=[str(number+10)])
    audio.save();return path


def remote(count=2,ident='100'):
    return dict(id=ident,artist='Artist',album_artists=['Artist'],title='Single',date='2020-01-01',type='EP',
                available=True,disc_count=1,track_count=count,tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,
                tracks=[dict(id=str(i+10),title=f'Song {i}',isrc=f'TEST{i}',duration=1,track_number=i,disc_number=1) for i in range(1,count+1)])


class ReleaseUpdateTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.base=Path(self.temp.name).resolve();self.root=self.base/'music';self.store=Store(self.base/'db')

    def rows(self,numbers,total=2):
        return [inspect_file(audio_file(self.root/'Artist'/'Single'/f'{n}.flac',n,total,f'Song {n}',f'TEST{n}'),self.root) for n in numbers]

    def test_exact_standalone_beats_rolling_and_cached_sibling(self):
        rows=self.rows([1,2]);small=remote();large=remote(3,'200')
        rows[0]['linked_ids']=dict(album_id='200',track_id='11')
        self.store.save_catalogue('a','GB',dict(releases=[large,small]))
        api=Mock();api.recording_releases.return_value=[]
        result=check_album_tags(rows,self.store,'GB',api)
        self.assertEqual({r['catalogue_choice']['id'] for r in result},{'100'})
        self.assertTrue(all(r['catalogue_choice']['structure']['compatible'] for r in result))
        self.assertLessEqual(api.recording_releases.call_count,2)

    def test_duplicate_exact_edition_links_while_rolling_release_is_rejected(self):
        rows=self.rows([1,2,3],3)
        for row in rows:row['tags']['album']=['Unbound']
        exact=remote(3,'435');exact.update(title='Unbound',album_artists=['Artist','Producer'],barcode='UPC-1')
        alias=copy.deepcopy(exact);alias['id']='553';alias['barcode']='QUALITY-VARIANT-UPC'
        for number,track in enumerate(alias['tracks'],1):track['id']=str(550+number)
        rolling=remote(4,'439');rolling.update(title='Unbound',album_artists=['Artist','Producer'],barcode='UPC-2')
        self.store.save_catalogue('a','GB',dict(releases=[rolling,alias,exact]))
        api=Mock();api.recording_releases.return_value=[]
        checked=check_album_tags(rows,self.store,'GB',api)
        for row in checked:
            compatible={str(o['id']) for o in row['catalogue_options'] if o['structure']['compatible']}
            self.assertEqual(compatible,{'435'})
            self.assertIn('release linked automatically',row['catalogue_note'])
            self.assertTrue(save_result(self.store,'GB',row))
        self.assertEqual({p[1]['ids']['album_id'] for p in saved_links(self.store,'GB').values()},{'435'})
        for payload in saved_links(self.store,'GB').values():
            self.assertEqual({p['album_id'] for p in payload[1]['equivalent_ids']},{'435','553'})
        linked=attach_links(rows,self.store,'GB')
        for row in linked:
            row['tags'].pop('bpm',None);row['tags'].pop('initialkey',None)
        api.track_tag_details.side_effect=lambda track:track
        enriched=check_album_tags(linked,self.store,'GB',api,enrich=True,
                                  dj_lookup=lambda track,album_id=None:({'bpm':124} if album_id=='435' else {}))
        self.assertTrue(all(row['metadata_changes'].get('bpm')==['124'] for row in enriched))

    def test_linking_reuses_configured_discovery_cache_window(self):
        rows=self.rows([1,2]);release=remote();release['tag_checked_at']=time.time()-2*86400
        self.store.save_catalogue('a','GB',dict(releases=[release]))
        api=Mock();api.album_tag_details.side_effect=AssertionError('Unexpired cache fetched again')
        checked=check_album_tags(rows,self.store,'GB',api)
        self.assertTrue(all(r.get('catalogue_choice') for r in checked))
        api.album_tag_details.assert_not_called()

    def test_equivalent_editions_prefer_local_product_code(self):
        rows=self.rows([1,2]);a=remote();a['barcode']='111'
        b=copy.deepcopy(a);b.update(id='200',barcode='222')
        for track in b['tracks']:track['id']='2'+track['id']
        for row in rows:row['tags']['upc']=['222']
        self.store.save_catalogue('a','GB',dict(releases=[a,b]))
        checked=check_album_tags(rows,self.store,'GB',Mock())
        self.assertEqual({r['catalogue_choice']['id'] for r in checked},{'200'})

    def test_equivalent_editions_require_recording_and_version_agreement(self):
        from library_manager.tag_review import _equivalent_edition_key
        a=remote();a['barcode']='111';b=copy.deepcopy(a);b['barcode']='222'
        self.assertEqual(_equivalent_edition_key(a),_equivalent_edition_key(b))
        b['tracks'][0]['title']+=' (Extended Mix)'
        self.assertNotEqual(_equivalent_edition_key(a),_equivalent_edition_key(b))
        b=copy.deepcopy(a);b['tracks'][0]['artists']=['Different guest']
        self.assertNotEqual(_equivalent_edition_key(a),_equivalent_edition_key(b))
        a['tracks'][0]['isrc']='';b=copy.deepcopy(a);b['barcode']='222'
        self.assertNotEqual(_equivalent_edition_key(a),_equivalent_edition_key(b))

    def test_online_match_labels_use_cached_release_and_same_format(self):
        from library_manager.linking import online_match_label,hydrate_match_labels
        release=remote();self.store.save_preferences('tag-review:GB:100',release)
        saved=dict(linked_ids=dict(album_id='100',track_id='11'))
        hydrate_match_labels([saved],self.store,'GB')
        choice=dict(catalogue_choice=dict(id='100',artist='Artist',album='Single',track_id='11'))
        self.assertEqual(online_match_label(saved),online_match_label(choice))
        self.assertEqual(online_match_label(saved),'Artist — Single [100]')

    def test_whole_release_recovers_wrong_positions_and_malformed_totals(self):
        rows=self.rows([1,2,3],3);release=remote(3)
        rows[0]['tags']['tracknumber']=['03'];rows[0]['tags']['tracktotal']=['01']
        rows[1]['tags']['tracknumber']=['01'];rows[1]['tags']['tracktotal']=['01']
        rows[2]['tags']['tracknumber']=['02'];rows[2]['tags']['tracktotal']=['01']
        evidence=structure(rows,release)
        self.assertTrue(evidence['compatible'])
        self.assertEqual(evidence['matched'],3)
        self.assertEqual(set(evidence['position_repairs']),{r['path'] for r in rows})
        self.assertEqual(set(evidence['total_repairs']),{r['path'] for r in rows})

    def test_track_total_accepts_release_or_per_disc_conventions(self):
        release=remote(4);release['disc_count']=2
        for n,track in enumerate(release['tracks']):
            track['disc_number']=1 if n<2 else 2;track['track_number']=n%2+1
        rows=self.rows([1,2],4)
        for row in rows:row['tags']['disctotal']=['2']
        self.assertTrue(structure(rows,release)['compatible'])
        for row in rows:row['tags']['tracktotal']=['2']
        self.assertTrue(structure(rows,release)['compatible'])

    def test_inconclusive_recheck_does_not_erase_existing_link(self):
        row=self.rows([1],1)[0]
        row['catalogue_choice']=dict(id='100',track_id='11',recording_verified=True,
                                     structure=dict(compatible=True),changes={})
        self.assertTrue(save_result(self.store,'GB',row))
        row.pop('catalogue_choice')
        row['catalogue_options']=[
            dict(id='200',track_id='21',recording_verified=True,structure=dict(compatible=True),changes={}),
            dict(id='300',track_id='31',recording_verified=True,structure=dict(compatible=True),changes={})]
        row['catalogue_note']='Two catalogue editions remain'
        self.assertTrue(save_result(self.store,'GB',row))
        payload=saved_links(self.store,'GB')[row['path']][1]
        self.assertEqual(payload['ids'],dict(album_id='100',track_id='11'))
        self.assertEqual(payload['status'],'linked')
        self.assertIn('inconclusive',payload['note'])

    def test_legacy_multiple_edition_review_is_rechecked_once(self):
        row=self.rows([1],1)[0]
        row['catalogue_options']=[
            dict(id='200',track_id='21',recording_verified=True,structure=dict(compatible=True),changes={}),
            dict(id='300',track_id='31',recording_verified=True,structure=dict(compatible=True),changes={})]
        row['catalogue_note']='Two verified release placements'
        self.assertTrue(save_result(self.store,'GB',row))
        saved=saved_links(self.store,'GB')[row['path']]
        legacy=copy.deepcopy(saved[1]);legacy.pop('linker_version')
        with self.store.connect() as db:
            db.execute('UPDATE track_links SET payload=? WHERE path=? AND market=?',
                       (json.dumps(legacy),row['path'],'GB'))
        self.assertEqual([r['path'] for r in pending_paths(self.store,'GB',[row])],[row['path']])
        self.assertTrue(save_result(self.store,'GB',row))
        self.assertEqual(pending_paths(self.store,'GB',[row]),[])

    def test_local_identity_change_rechecks_only_the_release_group(self):
        rows=self.rows([1,2]);other=inspect_file(audio_file(self.root/'Artist'/'Other'/'3.flac',3,3,'Song 3','TEST3',album='Other'),self.root)
        for row in rows+[other]:
            row['catalogue_choice']=dict(id='100',track_id=str(10+int(Path(row['path']).stem)),recording_verified=True,structure=dict(compatible=True),changes={})
            save_result(self.store,'GB',row)
        changed=copy.deepcopy(rows[0]);changed['tags']['album']=['Renamed']
        self.assertEqual(invalidate_related_links(self.store,'GB',rows[0],changed),2)
        attach_links(rows,self.store,'GB')
        self.assertTrue(all(r.get('recheck_required') for r in rows))
        self.assertFalse(saved_links(self.store,'GB')[other['path']][1].get('recheck_required'))

    def test_wrong_only_edition_does_not_become_a_central_link(self):
        rows=self.rows([1,2]);self.store.save_catalogue('a','GB',dict(releases=[remote(3)]))
        api=Mock();api.recording_releases.return_value=[]
        checked=check_album_tags(rows,self.store,'GB',api)
        self.assertNotIn('catalogue_choice',checked[0]);save_result(self.store,'GB',checked[0])
        self.assertEqual(saved_links(self.store,'GB')[rows[0]['path']][1]['ids'],{})

    def test_incomplete_album_positions_and_missing_tracks(self):
        rows=self.rows([1,2,5],10);release=remote(10)
        evidence=structure(rows,release)
        self.assertTrue(evidence['incomplete']);self.assertEqual(evidence['matched'],3)
        self.assertEqual([t['track_number'] for t in evidence['missing']],[3,4,6,7,8,9,10])
        self.assertFalse(structure(rows,remote(3))['compatible'])
        scan(self.store,self.root)
        covered=coverage(self.store.tracks(),dict(releases=[release]))[0]
        self.assertEqual(covered['state'],'Owned partial');self.assertEqual(len(covered['missing']),7)

    def test_duration_mix_isrc_and_disc_conflicts(self):
        local=dict(title='Song',isrc='TEST',duration=180)
        self.assertTrue(recording_matches(local,dict(local,id='1',duration=183)))
        self.assertFalse(recording_matches(local,dict(local,id='1',duration=183.01)))
        self.assertFalse(recording_matches(local,dict(local,id='1',title='Song (Extended Mix)')))
        self.assertFalse(recording_matches(local,dict(local,id='1',isrc='OTHER')))
        rows=self.rows([1],2);rows[0]['tags']['disctotal']=['2']
        self.assertIn('Disc total differs',structure(rows,remote())['conflicts'])

    def test_whole_release_sibling_conflict_prevents_single_file_auto_link(self):
        rows=self.rows([1,2]);rows[1]['tags']['isrc']=['WRONG']
        self.store.save_catalogue('a','GB',dict(releases=[remote()]))
        api=Mock();api.recording_releases.return_value=[]
        result=check_album_tags(rows[:1],self.store,'GB',api,context_rows=rows)
        self.assertNotIn('catalogue_choice',result[0])

    def test_unique_whole_release_links_without_saved_recording_ids(self):
        rows=self.rows([1,2])
        for row in rows:row['tags'].pop('isrc',None)
        self.store.save_catalogue('a','GB',dict(releases=[remote()]))
        api=Mock();api.recording_releases.return_value=[]
        checked=check_album_tags(rows,self.store,'GB',api)
        self.assertTrue(all(r.get('catalogue_choice',{}).get('id')=='100' for r in checked))
        self.assertTrue(all(r['catalogue_choice']['whole_release_verified'] for r in checked))
        self.assertTrue(save_result(self.store,'GB',checked[0]))
        self.assertEqual(saved_links(self.store,'GB')[checked[0]['path']][1]['ids']['album_id'],'100')

    def test_download_tags_paths_unicode_and_cover(self):
        stage=self.base/'stage';path=audio_file(stage/'temp.flac',1,120,disc=2,discs=2)
        audio=FLAC(path);audio.update(albumartist=['Cafe\u0301'],album=['Album'],title=['Science/Visions...'],date=['2024-05-01T00:00:00Z'],lyrics=['unwanted'])
        image=io.BytesIO();Image.new('RGB',(1280,1280),'blue').save(image,'JPEG')
        picture=Picture();picture.type=3;picture.mime='image/jpeg';picture.width=picture.height=1280;picture.data=image.getvalue();audio.add_picture(picture);audio.save()
        target=organise_download(path,stage,{},'11','100')
        self.assertEqual(target.relative_to(stage).as_posix(),'Café/Album (2024)/Disc 2/001 - Science - Visions.flac')
        self.assertEqual(str(target),unicodedata.normalize('NFC',str(target)))
        tags=FLAC(target);self.assertEqual(tags['artist'],['Artist','Guest']);self.assertEqual(tags['albumartist'],['Cafe\u0301'])
        self.assertEqual(tags['date'],['2024-05-01']);self.assertNotIn('lyrics',tags)
        self.assertEqual(Image.open(target.parent.parent/'cover.jpg').size,(1280,1280))
        output=self.base/'published';output.mkdir();files=publish_files(stage,output)
        self.assertEqual(len(files),2)
        bad=output/target.relative_to(stage);before=bad.read_bytes();target.unlink();target.write_bytes(b'collision')
        with self.assertRaises(ValueError):publish_files(stage,output)
        self.assertEqual(bad.read_bytes(),before)

    def test_single_disc_flat_and_existing_album_destination(self):
        stage=self.base/'stage';path=audio_file(stage/'temp.flac',2,10,'Song 2','TEST2')
        destination=dict(album_relative='Artist/Existing (2020)',disc_dirs={'1':'.'})
        target=organise_download(path,stage,{},'12','100',destination=destination)
        self.assertEqual(target.relative_to(stage).as_posix(),'Artist/Existing (2020)/02 - Song 2.flac')
        bad=audio_file(stage/'bad.flac')
        with self.assertRaises(ValueError):organise_download(bad,stage,{},'1','2',destination={'album_relative':'../escape'})
        self.assertTrue(bad.exists())

    def test_completed_downloads_inside_library_update_index_immediately(self):
        with self.store.connect() as db:db.execute('INSERT INTO roots VALUES(?,?,?)',(str(self.root),None,'ready'))
        inside=audio_file(self.root/'Artist'/'Album'/'1.flac')
        outside=audio_file(self.base/'outside.flac')
        self.assertEqual(index_downloaded_files(self.store,[str(inside),str(outside),str(self.root/'cover.jpg')]),1)
        self.assertEqual([t['path'] for t in self.store.tracks()],[str(inside.resolve())])

    def signal(self,hits=3):
        samples=np.zeros((12000,2),dtype=np.int32)
        for start in [500,2500,4500][:hits]:
            samples[start:start+36,0]=np.array(list(PATTERN),dtype=np.int32)<<16
            samples[start+38:start+42,0]=np.array([0,0,1,1],dtype=np.int32)<<16
        return samples

    def test_mqa_requires_repeated_signal_and_survives_flac_decode(self):
        self.assertIsNone(scan_samples(self.signal(1)))
        self.assertIsNone(scan_samples(np.random.default_rng(7).integers(-2**31,2**31-1,(132300,2),dtype=np.int32)))
        self.assertEqual(scan_samples(self.signal())['original_rate'],96000)
        path=self.base/'mqa.flac';sf.write(path,self.signal(),44100,subtype='PCM_24')
        result=audit_file(path);self.assertEqual(result['status'],'MQA signal');self.assertEqual(result['bits'],24)
        self.assertEqual(result['rate'],44100);self.assertEqual(result['original_rate'],96000)

    def test_mqa_tags_are_not_proof_and_cache_invalidates(self):
        path=audio_file(self.root/'song.flac');audio=FLAC(path);audio['originalsamplerate']=['96000'];audio.save()
        self.assertEqual(audit_file(path)['status'],'Metadata clue')
        audio['mqaencoder']=['MQA'];audio.save();scan(self.store,self.root)
        result=audit_library(self.store,str(self.root));self.assertEqual(result[0]['status'],'MQA tags')
        attached=attach_links([inspect_file(path,self.root)],self.store,'GB')[0]
        self.assertEqual(attached['mqa_audit']['status'],'MQA tags')
        with patch('library_manager.mqa_audit.audit_file',side_effect=AssertionError('Should reuse cache')):audit_library(self.store,str(self.root))
        audio['comment']=['changed'];audio.save()
        with patch('library_manager.mqa_audit.audit_file',wraps=audit_file) as inspect:
            audit_library(self.store,str(self.root));inspect.assert_called_once()

    def prepare_optimization(self):
        self.rows([1],1);scan(self.store,self.root)
        self.store.mapping('Artist','a','confirmed','test',True)
        release=remote(2);release['date']='2021-01-01'
        for track in release['tracks']:track['artists']=['Artist','Guest']
        self.store.save_catalogue('a','GB',dict(id='a',releases=[release]))
        plans=find_optimizations(self.store,'GB',self.root);self.assertEqual(len(plans),1)
        return plans[0]

    def replacements(self,plan):
        files=[str(audio_file(self.root/'Artist'/'Album'/f'{n}.flac',n,2,f'Song {n}',f'TEST{n}',album='Album',album_id='100')) for n in (1,2)]
        self.store.enqueue(dict(plan['release'],downloaded_files=files))
        with self.store.connect() as db:db.execute("UPDATE queue SET decision='downloaded'")
        return files

    def test_optimizer_exclusive_mix_and_unreleased_rejected(self):
        plan=self.prepare_optimization();release=copy.deepcopy(plan['release'])
        release['tracks'][0]['title']='Song 1 (Radio Edit)'
        self.store.save_catalogue('a','GB',dict(id='a',releases=[release]))
        self.assertEqual(find_optimizations(self.store,'GB',self.root),[])
        release=copy.deepcopy(plan['release']);release['date']='2999-01-01'
        self.store.save_catalogue('a','GB',dict(id='a',releases=[release]))
        self.assertEqual(find_optimizations(self.store,'GB',self.root),[])

    def test_online_optimizer_refreshes_legacy_track_lists_once(self):
        from library_manager.optimizations import check_candidate_releases
        self.rows([1]);scan(self.store,self.root)
        self.store.mapping('Artist','a','confirmed','fixture',True)
        release=remote();self.store.save_catalogue('a','GB',dict(releases=[release]))
        self.store.save_preferences('tag-review:GB:100',release)
        refreshed=copy.deepcopy(release)
        for track in refreshed['tracks']:track['artists']=['Artist','Guest']
        api=Mock();api.album_tag_details.return_value=refreshed
        self.assertEqual(check_candidate_releases(self.store,'GB',self.root,api),1)
        self.assertEqual(check_candidate_releases(self.store,'GB',self.root,api),0)
        api.album_tag_details.assert_called_once()

    def test_optimizer_prefers_complete_local_album_before_remote(self):
        audio_file(self.root/'Artist'/'Single'/'1.flac',1,1,'Song 1','TEST1',album='Single')
        audio_file(self.root/'Artist'/'Album'/'1.flac',1,2,'Song 1','TEST1',album='Album')
        audio_file(self.root/'Artist'/'Album'/'2.flac',2,2,'Song 2','TEST2',album='Album')
        scan(self.store,self.root)
        plans=find_optimizations(self.store,'GB',self.root)
        self.assertEqual(len(plans),1);self.assertEqual(plans[0]['kind'],'local')
        self.assertEqual(Path(plans[0]['target_folder']).name,'Album')
        self.assertEqual(len(validate_consolidation(self.store,plans[0],decode=False)),2)
        self.store.mapping('Artist','a','confirmed','fixture',True)
        release=remote(3);release['title']='Bigger album'
        for track in release['tracks']:track['artists']=['Artist','Guest']
        self.store.save_catalogue('a','GB',dict(releases=[release]))
        self.assertEqual({p['kind'] for p in find_optimizations(self.store,'GB',self.root,scope='local')},{'local'})
        online=find_optimizations(self.store,'GB',self.root,scope='remote')
        self.assertEqual({p['kind'] for p in online},{'remote'})
        self.assertTrue(any(Path(p['folder']).name=='Single' for p in online))


    def test_consolidation_requires_complete_files_and_retains_original_on_trash_error(self):
        plan=self.prepare_optimization();trash=Mock(side_effect=ValueError('Trash unavailable'))
        with self.assertRaises(ValueError):consolidate(self.store,'GB',plan,trash)
        trash.assert_not_called();self.replacements(plan)
        self.assertEqual(len(validate_consolidation(self.store,plan)),2)
        with self.assertRaisesRegex(ValueError,'Trash unavailable'):consolidate(self.store,'GB',plan,trash)
        self.assertTrue(Path(plan['folder']).exists());self.assertTrue(all(Path(t['path']).exists() for t in plan['sources']))

    def test_consolidation_stale_extras_and_truncated_replacement_blocked(self):
        plan=self.prepare_optimization();files=self.replacements(plan)
        extra=Path(plan['folder'])/'unrelated.txt';extra.write_text('keep')
        with self.assertRaisesRegex(ValueError,'additional files'):validate_consolidation(self.store,plan)
        extra.unlink();p=Path(files[0]);p.write_bytes(p.read_bytes()[:-8])
        with self.assertRaises((ValueError,RuntimeError)):validate_consolidation(self.store,plan)
        self.assertTrue(Path(plan['folder']).exists())

    def test_consolidation_transfers_links_and_only_uses_trash(self):
        plan=self.prepare_optimization();files=self.replacements(plan);trash_folder=self.base/'mock-trash'
        def trash(path):Path(path).rename(trash_folder)
        self.assertEqual(consolidate(self.store,'GB',plan,trash),1)
        self.assertTrue(trash_folder.is_dir());self.assertEqual(len(self.store.tracks()),2)
        self.assertEqual({v[1]['ids']['album_id'] for v in saved_links(self.store,'GB').values()},{'100'})
        self.assertTrue(all(Path(p).exists() for p in files))

    def test_recommendations_use_evidence_not_missing_fields(self):
        local=[dict(label='Label',copyright='2020 Label')]
        r=dict(label='Label',copyright='2024 Label',album_artists=['Artist'])
        self.assertEqual(recommend(r,local,['Artist'])['badge'],'Recommended')
        self.assertEqual(recommend({},local,['Artist'])['badge'],'Potential')
        self.assertEqual(recommend(dict(album_artists=['Other'],type='COMPILATION'),local,['Artist'])['badge'],'Unmatched')

    def test_recommendations_filter_without_dropping_suspect_candidates(self):
        releases=[]
        for ident,badge in [('1','Recommended'),('2','Suspect'),('3','Unmatched')]:
            rel=dict(id=ident,title='Release '+ident,artist='Artist',date='2024-01-01',type='SINGLE',available=True,
                     link_checked_at=time.time(),track_count=1,explicit=False)
            releases.append(dict(release=rel,artist_ids=['a'],local_artists=['Artist'],state='Missing release',recommendation=dict(badge=badge,evidence=[])))
        data=dict(compared=releases,owned_dates={'a':{'2020-01-01'}},link_cache_days=30)
        _,visible,_=filter_coverage(data,{},('All dates','','All statuses','All types','All copyrights','All recommendations'))
        self.assertEqual({r['release']['id'] for r in visible},{'1','2','3'})
        _,suspect,_=filter_coverage(data,{},('All dates','','All statuses','All types','All copyrights','Suspect / Low match'))
        self.assertEqual([r['release']['id'] for r in suspect],['2'])

    def test_background_pending_state_is_idempotent_until_inputs_change(self):
        row=self.rows([1],1)[0]
        scan(self.store,self.root)
        row['catalogue_note']='No match';save_result(self.store,'GB',row)
        self.assertEqual(pending_paths(self.store,'GB',[row]),[])
        before=state_signature(self.store,'GB',self.root)
        with self.store.connect() as db:db.execute('UPDATE local_files SET mtime=mtime+1 WHERE path=?',(row['path'],))
        self.assertNotEqual(before,state_signature(self.store,'GB',self.root))


class BridgeUpdateTests(unittest.TestCase):
    def test_album_pagination_and_repeated_page_guard(self):
        from types import SimpleNamespace as Obj
        from library_manager.download_bridge import load_album_tracks
        tracks=[Obj(id=i) for i in range(105)]
        album=Obj(num_tracks=105,tracks=Mock(side_effect=lambda limit,offset:tracks[offset:offset+limit]))
        self.assertEqual(len(load_album_tracks(album,pause=lambda _:None)),105)
        self.assertEqual(album.tracks.call_args.kwargs,dict(limit=100,offset=100))
        album.tracks=Mock(return_value=tracks[:100])
        with self.assertRaisesRegex(ValueError,'Repeated'):load_album_tracks(album,pause=lambda _:None)

    def test_mocked_tidaler_download_pipeline(self):
        import sys
        from types import SimpleNamespace as Obj,ModuleType
        from library_manager import download_bridge as bridge
        with tempfile.TemporaryDirectory() as directory:
            output=Path(directory).resolve()/'output';events=[];qualities=[]
            artist=Obj(name='Artist');guest=Obj(name='Guest')
            album=Obj(id='100',name='Album',version=None,artists=[artist],num_tracks=2,num_volumes=1,available_release_date=None)
            tracks=[Obj(id=str(n+10),name=f'Song {n}',full_name=f'Song {n} (Extended Mix)',artists=[artist,guest],track_num=n,volume_num=1,album=album,available=True) for n in (1,2)]
            album.tracks=lambda limit,offset:tracks[offset:offset+limit]
            class Settings:
                def __init__(self):self.data=Obj()
            class Tidal:
                def __init__(self,settings):
                    self.session=Obj(album=lambda ident:album)
                    assert settings.data.lyrics_embed is False
                def login_token(self):return True
            class Download:
                def __init__(self,**kwargs):self.stage=Path(kwargs['path_base'])
                def item(self,**kwargs):
                    track=kwargs['media'];qualities.append(kwargs['quality_audio'])
                    path=audio_file(self.stage/(kwargs['file_template']+'.flac'),track.track_num,2,title=track.name,isrc=f'TEST{track.track_num}')
                    picture=Picture();picture.type=3;picture.mime='image/jpeg';picture.width=picture.height=1280
                    art=io.BytesIO();Image.new('RGB',(1280,1280),'red').save(art,'JPEG');picture.data=art.getvalue()
                    audio=FLAC(path);audio.add_picture(picture);audio.save();return True,path
            modules={}
            for name in ('tidaler','tidaler.config','tidaler.download','tidaler.constants','tidalapi','rich','rich.progress'):modules[name]=ModuleType(name)
            modules['tidaler.config'].Settings=Settings;modules['tidaler.config'].Tidal=Tidal;modules['tidaler.download'].Download=Download
            modules['tidaler.constants'].CoverDimensions=Obj(Px1280=1280,Px640=640,Px320=320,PxORIGIN='origin')
            modules['tidalapi'].Quality=Obj(high_lossless='LOSSLESS',hi_res_lossless='HIRES',high='HIGH',low='LOW')
            modules['rich.progress'].Progress=lambda **kw:Obj(tasks=[])
            request=dict(output=str(output),settings={},items=[dict(id='100',release=dict(artist='Artist',title='Album',track_count=2,selected_tracks=[dict(id='12')]))])
            with patch.dict(sys.modules,modules),patch.object(sys,'stdin',io.StringIO(json.dumps(request)+'\n')),patch.object(bridge,'emit',side_effect=lambda event,**data:events.append(dict(event=event,**data))),patch.object(bridge.signal,'signal'):
                self.assertEqual(bridge.main(),0)
            self.assertEqual(qualities,['LOSSLESS'])
            files=list(output.rglob('*.flac'));self.assertEqual(len(files),1)
            tags=FLAC(files[0]);self.assertEqual(tags['title'],['Song 2 (Extended Mix)']);self.assertEqual(tags['artist'],['Artist','Guest']);self.assertEqual(tags['albumartist'],['Artist'])
            self.assertEqual(tags['tracktotal'],['02']);self.assertEqual(tags['tidal_album_id'],['100'])
            self.assertTrue((files[0].parent/'cover.jpg').is_file());self.assertNotIn('Disc',str(files[0]))
            completed=next(e for e in events if e['event']=='completed');self.assertIn(str(files[0]),completed['files'])


class M4ACompatibilityTests(unittest.TestCase):
    @unittest.skipUnless(Path('/usr/bin/afconvert').exists(),'macOS audio converter required')
    def test_existing_aac_quality_options_keep_metadata_and_paths(self):
        import subprocess
        from mutagen.mp4 import MP4
        from mutagen.easymp4 import EasyMP4
        with tempfile.TemporaryDirectory() as tmp:
            stage=Path(tmp).resolve();wav=stage/'source.wav';path=stage/'source.m4a'
            sf.write(wav,np.zeros((44100,2)),44100,subtype='PCM_16')
            subprocess.run(['/usr/bin/afconvert','-f','m4af','-d','aac','-b','128000',str(wav),str(path)],check=True,capture_output=True)
            audio=EasyMP4(path);audio.update(title=['Song'],album=['Album'],albumartist=['Artist'],artist=['Performer'],date=['2020'],tracknumber=['1/2'],discnumber=['1/1']);audio.save()
            target=organise_download(path,stage,{},'11','100',metadata={'initialkey':['Am']})
            self.assertEqual(target.relative_to(stage).as_posix(),'Artist/Album (2020)/01 - Song.m4a')
            easy=EasyMP4(target);raw=MP4(target)
            self.assertEqual(easy['tracknumber'],['1/2']);self.assertEqual(easy['albumartist'],['Artist'])
            self.assertEqual(bytes(raw['----:com.apple.iTunes:initialkey'][0]),b'8A')
            self.assertEqual(bytes(raw['----:com.apple.iTunes:tidal_track_id'][0]),b'11')

if __name__=='__main__':unittest.main()
