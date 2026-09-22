import unittest
from unittest.mock import Mock,patch
from library_manager.link_statistics import unresolved_paths
from test_selection_and_identity import SelectionIdentityTests

class ScopeTests(unittest.TestCase):
    def test_linked_equivalent_editions_and_ignored_are_never_automatic_targets(self):
        store=Mock();store.ignored_local_files.return_value={'ignored'}
        rows=[dict(path=p,catalogue_options=[{},{}]) for p in ('linked','ignored','choice')]
        rows += [dict(path='limit',catalogue_note='Too many editions'),dict(path='missing')]
        with patch('library_manager.link_statistics.link_statistics',return_value={'active':{'linked':{'track_id':'1','album_id':'2'}}}):
            self.assertEqual(unresolved_paths(store,'GB',rows,True),{'choice','limit'})
            self.assertEqual(unresolved_paths(store,'GB',rows),{'choice','limit','missing'})

class ActionTests(SelectionIdentityTests):
    def test_edition_and_unlinked_actions_are_explicitly_scoped(self):
        w=self.window();w.link_plan=[{'path':'choice'}]
        with patch('library_manager.link_statistics.unresolved_paths',return_value={'choice'}) as targets,patch.object(w,'start_linking') as start:
            w.recheck_multiple_editions()
            targets.assert_called_once_with(self.store,w.market,w.link_plan,editions_only=True)
            start.assert_called_once_with(recheck=True,target_paths={'choice'})
            start.reset_mock();w.recheck_unlinked_tracks()
            start.assert_called_once_with(recheck=True,target_paths={'choice'})
            self.assertEqual(w.link_filter.currentText(),'Unlinked tracks')
        with patch('library_manager.link_statistics.unresolved_paths',return_value=set()),patch.object(w,'start_linking') as start:
            w.recheck_multiple_editions();w.recheck_unlinked_tracks();start.assert_not_called()

def load_tests(loader,tests,pattern):
    return unittest.TestSuite([loader.loadTestsFromTestCase(ScopeTests),loader.loadTestsFromName(__name__+'.ActionTests.test_edition_and_unlinked_actions_are_explicitly_scoped')])
