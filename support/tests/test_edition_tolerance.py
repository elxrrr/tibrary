import copy,unittest
from library_manager.tag_review import _collapse_equivalent_editions
from library_manager.maintenance import track_changes
from test_release_update import remote

class ToleranceTests(unittest.TestCase):
    def test_near_durations_keep_aliases_but_not_clean_editions_or_chains(self):
        details={};opts=[]
        for ident,offset,explicit in [('1',0,True),('2',1,True),('3',4,True),('4',0,False)]:
            r=remote(2,ident);r['explicit']=explicit
            for t in r['tracks']:t['duration']=100+offset
            details[ident]=r;opts.append(dict(id=ident,track_id=ident+'0',changes={}))
        result=_collapse_equivalent_editions(opts,details)
        self.assertEqual(len(result),3)
        self.assertEqual({p['album_id'] for p in result[0]['equivalent_placements']},{'1','2'})
    def test_impossible_track_total_is_reported_not_guessed(self):
        changes,issue=track_changes({'tracknumber':['05'],'tracktotal':['01']})
        self.assertFalse(changes);self.assertIn('exceeds total',issue)
