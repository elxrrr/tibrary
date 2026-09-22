import copy,unittest
from library_manager.number_repairs import number_repairs
from test_selection_and_identity import SelectionIdentityTests
from library_manager.library_workflows import workflow_plan


def row(n,total='01',disc='01'):
 return dict(path=f'/music/Artist/Album/{n}.flac',root='/music',blocked='',stamp=(1,2),tags={'albumartist':['Artist'],'album':['Album'],'title':[f'Track {n}'],'tracknumber':[str(n)],'tracktotal':[total],'discnumber':[disc],'disctotal':['01']})

def option(r,total):
 return dict(album='Album',recording_verified=True,changes={'tracktotal':[str(total)]},structure=dict(compatible=True,alignments={r['path']:dict(disc_number=1,track_number=int(r['tags']['tracknumber'][0]))}))

class NumberTests(unittest.TestCase):
 def test_local_consensus_repairs_without_assuming_inventory_complete(self):
  a,b=row(5),row(10,'15');edits,notes=number_repairs([a,b])[a['path']]
  self.assertEqual(edits['tracktotal'],['15']);self.assertEqual(edits['tracknumber'],['05'])
  self.assertNotIn('tracktotal',number_repairs([a])[a['path']][0])
 def test_cached_candidates_must_agree_and_align(self):
  a=row(5);a['catalogue_options']=[option(a,15),option(a,15)]
  self.assertEqual(number_repairs([a])[a['path']][0]['tracktotal'],['15'])
  a['catalogue_options'].append(option(a,16))
  self.assertNotIn('tracktotal',number_repairs([a])[a['path']][0])
  a['catalogue_options']=[option(a,15)];a['catalogue_options'][0]['structure']['alignments'][a['path']]['track_number']=6
  self.assertNotIn('tracktotal',number_repairs([a])[a['path']][0])
 def test_verified_counts_do_not_require_a_tag_proposal(self):
  a=row(5);candidate=option(a,15);candidate['changes']={};candidate['release_counts']={'tracktotal':15,'disctotal':1}
  a['catalogue_options']=[candidate]
  self.assertEqual(number_repairs([a])[a['path']][0]['tracktotal'],['15'])
 def test_operation_is_preview_only_and_isolated(self):
  a=row(5);a['catalogue_options']=[option(a,15)];before=copy.deepcopy(a)
  plan=workflow_plan([a],'tags',dates=False,discs=True,keys=False)
  self.assertEqual(plan[0]['changes']['tracktotal'],['15']);self.assertEqual(plan[0]['target'],a['path']);self.assertEqual(a,before)
  other=workflow_plan([a],'tags',dates=True,discs=False,keys=False)
  self.assertNotIn('tracktotal',other[0]['changes'])

class CountTests(SelectionIdentityTests):
 def test_track_only_padding_is_counted(self):
  w=self.window();a=row(5,'15');w.update_action_counts([a])
  self.assertIn('1',w.tools_discs.text())

def load_tests(loader,tests,pattern):
 return unittest.TestSuite([loader.loadTestsFromTestCase(NumberTests),loader.loadTestsFromName(__name__+'.CountTests.test_track_only_padding_is_counted')])
