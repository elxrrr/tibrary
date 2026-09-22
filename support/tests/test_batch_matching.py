import tempfile
import unittest
from pathlib import Path
from library_manager.core import Store
from library_manager.matching import BatchMatcher, accepts, score_candidates
from library_manager.tidal import CatalogueError


def artist(ident, name, albums=3):
    return dict(id=ident,name=name,releases=[dict(id=f'{ident}-{i}',artist=name,title=f'Release {i}',date='2024-01-01',type='ALBUM',available=True,track_count=10,tracks=[],tracks_loaded=False) for i in range(albums)])


def tracks():
    return [dict(album=f'Release {i}',title='Track') for i in range(3)]


class BatchTests(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory();self.addCleanup(self.directory.cleanup)
        self.store=Store(Path(self.directory.name)/'app.sqlite3')
        self.calls=[];self.favourite_calls=[]
        outer=self
        class Api:
            def artist(self, ident, progress): outer.calls.append(ident);return artist(ident,ident)
            def search(self,name):return [{'id':name,'name':name}]
        self.api=Api()
    def matcher(self):
        def favourites(cancel,progress):self.favourite_calls.append(1);return []
        return BatchMatcher(self.store,self.api,'GB',favourites)

    def test_multiple_artists_persist_and_load_favourites_once(self):
        self.matcher().run([('A',tracks()),('B',tracks())])
        self.assertEqual(len(self.favourite_calls),1);self.assertEqual(self.calls,['A','B'])
        reopened=Store(self.store.path)
        self.assertEqual([r['status'] for r in reopened.rows('SELECT * FROM match_reviews ORDER BY artist')],['auto','auto'])
        self.assertEqual(len(reopened.rows('SELECT * FROM mappings')),2)

    def test_multiple_positive_candidates_linked_and_batch_continues(self):
        self.api.search=lambda name:[dict(id=name+'1',name=name),dict(id=name+'2',name=name)] if name=='A' else [dict(id=name,name=name)]
        self.matcher().run([('A',tracks()),('B',tracks())])
        self.assertEqual([r['status'] for r in self.store.rows('SELECT * FROM match_reviews ORDER BY artist')],['auto','auto'])

    def test_cancel_and_resume_does_not_repeat_finished_artist(self):
        cancel=[False]
        self.matcher().run([('A',tracks()),('B',tracks())],cancel=lambda:cancel[0],updated=lambda name:cancel.__setitem__(0,True))
        self.assertEqual(self.calls,['A'])
        self.matcher().run([('A',tracks()),('B',tracks())],resume=True)
        self.assertEqual(self.calls,['A','B'])

    def test_manual_confirmation_and_unlink_never_overwritten(self):
        self.store.mapping('A','manual','confirmed','User',True)
        self.store.mapping('B',None,'unlinked','User',True)
        self.matcher().run([('A',tracks()),('B',tracks())])
        self.assertFalse(self.calls);self.assertFalse(self.favourite_calls)
        self.assertEqual(self.store.rows("SELECT tidal_id FROM mappings WHERE artist='A'")[0]['tidal_id'],'manual')

    def test_access_failure_stops_without_hammering_remaining_artists(self):
        def denied(name): raise CatalogueError('Denied',status=403)
        self.api.search=denied
        result=self.matcher().run([('A',tracks()),('B',tracks())])
        self.assertIn('1 remaining',result)
        self.assertEqual(len(self.store.rows('SELECT * FROM match_reviews')),1)

    def test_positive_matches_ignore_legacy_threshold_but_name_only_does_not_match(self):
        preferences=dict(enabled=True,threshold=50,margin=10)
        self.assertFalse(accepts([dict(score=99,summary_releases=0)],preferences))
        strong=dict(score=80,matched_releases=3,exact_name=True,artist=dict(name='A'))
        self.assertTrue(accepts([strong,strong],preferences))
        self.assertTrue(accepts([strong],preferences))
        self.assertFalse(accepts([strong],dict(preferences,enabled=False)))

    def test_legacy_threshold_is_retained_but_does_not_block_positive_matches(self):
        self.store.save_match_preferences(dict(enabled=True,threshold=90,margin=25))
        self.matcher().run([('A',tracks())])
        self.assertEqual(self.store.rows('SELECT status FROM match_reviews')[0]['status'],'auto')
        self.assertEqual(Store(self.store.path).match_preferences()['threshold'],90)
        self.store.save_match_preferences(dict(enabled=True,threshold=75,margin=25))
        self.matcher().run([('A',tracks())])
        self.assertEqual(self.store.rows('SELECT status FROM match_reviews')[0]['status'],'auto')
        self.assertEqual(self.calls,['A'])

    def test_network_failure_pauses_instead_of_timing_out_each_artist(self):
        calls=[]
        def offline(name):
            calls.append(name)
            raise CatalogueError('Offline',batch_fatal=True)
        self.api.search=offline
        result=self.matcher().run([('A',tracks()),('B',tracks())])
        self.assertEqual(calls,['A'])
        self.assertIn('Paused',result)
        self.assertIn('1 remaining',result)

    def test_match_all_retries_review_but_keeps_accepted(self):
        self.matcher().run([('A',tracks())])
        self.calls.clear()
        self.store.save_match_review('B','review',[],'GB')
        self.matcher().run([('A',tracks()),('B',tracks())],resume=True)
        self.assertEqual(self.calls,['B'])

    def test_broad_search_prioritises_punctuation_equivalent_and_keeps_choices(self):
        self.api.search=lambda name:[dict(id=str(i),name=f'Other {i}') for i in range(15)]+[dict(id='right',name='1788 L')]
        scored,warning=self.matcher().candidates('1788-L',tracks(),lambda:False,lambda message:None)
        self.assertEqual(self.calls[0],'right')
        self.assertEqual(len(self.calls),6)
        self.assertEqual(len(scored),16)
        self.assertEqual(scored[0]['artist']['id'],'right')
        self.assertFalse(warning)
        self.assertEqual(sum(bool(s['artist'].get('unverified_summary')) for s in scored),10)

    def test_one_track_can_produce_reviewable_release_match(self):
        self.matcher().run([('A',tracks()[:1])])
        review=self.store.rows('SELECT * FROM match_reviews')[0]
        self.assertEqual(review['status'],'auto')
        self.assertIn('1/1 local release titles',review['payload'])

    def test_two_complete_releases_and_many_releases_auto_accept(self):
        for count,expected in ((2,85),(11,98)):
            local=[dict(album=f'Release {i}',title='Song') for i in range(count)]
            scores=score_candidates('Alabama Shakes',local,[artist('1','Alabama Shakes',count)])
            self.assertEqual(scores[0]['score'],expected)
            self.assertTrue(accepts(scores,self.store.match_preferences()))

    def test_partial_and_competing_positive_matches_qualify(self):
        scores=score_candidates('A',tracks(),[artist('1','A',1)])
        self.assertTrue(accepts(scores,self.store.match_preferences()))
        scores=score_candidates('A',tracks(),[artist('1','A'),artist('2','A')])
        self.assertTrue(accepts(scores,self.store.match_preferences()))

    def test_unchecked_candidates_do_not_block_checked_positive_match(self):
        for other,accepted in (('1788 L',True),('Unrelated musician',True)):
            scores=score_candidates('1788-L',tracks(),[artist('1','1788-L'),dict(id='2',name=other,releases=[],unverified_summary=True)])
            self.assertEqual(accepts(scores,self.store.match_preferences()),accepted)

    def test_generic_titles_and_duplicate_tracks_cannot_inflate_evidence(self):
        local=[dict(album='Greatest Hits',title='Song')]*20
        candidate=artist('1','A',1);candidate['releases'][0]['title']='Greatest Hits'
        scores=score_candidates('A',local,[candidate])
        self.assertFalse(accepts(scores,self.store.match_preferences()))
        scores=score_candidates('A',tracks()[:1]*20,[artist('1','A')])
        self.assertEqual(scores[0]['score'],75)

    def test_single_release_winner_not_blocked_by_fuzzy_search_suggestions(self):
        for name,other in [('A. G. Cook','A.Cook'),('aespa','Aespar'),('AC Slater','Ace Slater')]:
            with self.subTest(name=name):
                candidates=[artist('winner',name,1),dict(id='suggestion',name=other,releases=[],unverified_summary=True)]
                scores=score_candidates(name,tracks()[:1],candidates)
                self.assertTrue(accepts(scores,self.store.match_preferences()))
                candidates[1]['name']=name
                self.assertTrue(accepts(score_candidates(name,tracks()[:1],candidates),self.store.match_preferences()))
                candidates[1]=artist('competitor',name,1)
                self.assertTrue(accepts(score_candidates(name,tracks()[:1],candidates),self.store.match_preferences()))
