import unittest,time
from unittest.mock import Mock
from library_manager.link_statistics import matches_link_filter
from library_manager.tag_review import check_album_tags
from library_manager.maintenance import inspect_file
from test_selection_and_identity import SelectionIdentityTests
from test_release_update import audio_file,remote

class FilterTests(unittest.TestCase):
    def test_filters_use_active_links_not_stale_row_flags(self):
        linked=dict(path='linked',catalogue_options=[{},{}])
        choice=dict(path='choice',status='linked',linked_ids={'track_id':'stale'},catalogue_options=[{'structure':{'compatible':False}}])
        missing=dict(path='missing');ignored=dict(path='ignored',catalogue_options=[{}])
        rows=[linked,choice,missing,ignored];active={'linked':{}};excluded={'ignored'}
        expected={'All files':{'linked','choice','missing','ignored'},'Needs attention':{'choice','missing'},'Unlinked tracks':{'choice','missing'},'Linked tracks':{'linked'},'Too many editions / Needs choice':{'choice'},'Ignored files':{'ignored'}}
        for name,paths in expected.items():
            self.assertEqual({r['path'] for r in rows if matches_link_filter(r,name,active,excluded)},paths,name)

class CacheTests(SelectionIdentityTests):
    def test_details_cached_outside_artist_catalogue_resolve_locally(self):
        rows=[inspect_file(audio_file(self.root/f'{i}.flac',i,2,f'Song {i}',f'TEST{i}'),self.root) for i in (1,2)]
        release=remote();release['tag_checked_at']=time.time()-86400*3
        self.store.save_preferences('tag-review:GB:100',release)
        api=Mock()
        result=check_album_tags(rows,self.store,'GB',api,cache_first=True)
        self.assertTrue(all(r.get('catalogue_choice') for r in result))
        self.assertEqual(api.mock_calls,[])
    def test_link_evidence_can_repair_impossible_total_without_tag_proposal(self):
        from library_manager.number_repairs import number_repairs
        rows=[inspect_file(audio_file(self.root/f'{i}.flac',i,2,f'Song {i}',f'TEST{i}'),self.root) for i in (1,2)]
        for row in rows:row['tags']['tracktotal']=['01']
        release=remote();release['tag_checked_at']=time.time()
        self.store.save_preferences('tag-review:GB:100',release)
        result=check_album_tags(rows,self.store,'GB',Mock(),cache_first=True)
        self.assertTrue(all(r.get('catalogue_choice') for r in result))
        self.assertEqual(number_repairs(result)[rows[1]['path']][0]['tracktotal'],['02'])
        # Existing saved associations gain counts from the cache without a relink.
        from library_manager.linking import hydrate_match_labels
        for row in result:
            for candidate in [*row['catalogue_options'],row['catalogue_choice']]:
                candidate.pop('release_counts',None);candidate['changes'].pop('tracktotal',None)
        hydrate_match_labels(result,self.store,'GB')
        self.assertEqual(number_repairs(result)[rows[1]['path']][0]['tracktotal'],['02'])
    def test_release_details_saved_for_linker_without_artist_owner(self):
        w=self.window();r=remote();w.save_release_detail({},r)
        self.assertEqual(self.store.preferences(f'tag-review:{w.market}:100')['tracks'],r['tracks'])

def load_tests(loader,tests,pattern):
 return unittest.TestSuite([loader.loadTestsFromTestCase(FilterTests),loader.loadTestsFromNames([__name__+'.CacheTests.'+n for n in CacheTests.__dict__ if n.startswith('test_')])])
