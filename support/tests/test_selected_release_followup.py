import copy,unittest
from unittest.mock import patch,Mock
from PySide6.QtCore import Qt,QTimer
from PySide6.QtWidgets import QMenu
from test_selection_and_identity import SelectionIdentityTests,app
from test_release_update import remote,audio_file
from library_manager.maintenance import inspect_file
from library_manager.release_matching import structure,recording_matches,replacement_matches
from library_manager.tag_review import check_album_tags
from library_manager.ui import OnlineAlbumLinkDialog

class ReleaseFollowupTests(SelectionIdentityTests):
    def test_complete_edition_recovers_omitted_live_label_without_relaxing_pruning(self):
        rows=[inspect_file(audio_file(self.root/'Artist/Single'/f'{n}.flac',n,2,f'Song {n}',f'TEST{n}'),self.root) for n in (1,2)]
        release=remote();release['tracks'][1]['title']='Song 2 (Live from Studio A)'
        self.assertFalse(recording_matches(rows[1],release['tracks'][1]))
        self.assertFalse(replacement_matches(rows[1],release['tracks'][1]))
        self.assertTrue(structure(rows,release)['compatible'])
        self.assertFalse(structure(rows[1:],release)['compatible'])
        bad=copy.deepcopy(rows);bad[0]['duration']=100
        self.assertFalse(structure(bad,release)['compatible'])
        bad=copy.deepcopy(rows);bad[1]['tags']['title']=['Song 2 (Studio Mix)']
        self.assertFalse(structure(bad,release)['compatible'])
        self.store.save_catalogue('a','GB',dict(releases=[release]))
        api=Mock();api.recording_releases.return_value=[]
        checked=check_album_tags(rows,self.store,'GB',api)
        self.assertTrue(all(r.get('catalogue_choice') for r in checked))

    def test_context_recheck_uses_only_selected_paths(self):
        w=self.window();w.nav.setCurrentRow(2)
        chosen=[dict(path='/test/selected.flac')]
        with patch.object(w,'link_selected',return_value=chosen),patch.object(w,'start_linking') as start:
            w.recheck_selected_tracks()
            start.assert_called_once_with(recheck=True,target_paths={'/test/selected.flac'})

    def test_targeted_worker_skips_artist_batch_and_keeps_release_context(self):
        w=self.window();w.demo_mode=False;w.market='GB';root=str(self.root)
        w.link_releases_root.addItem(root,root);w.link_releases_root.setCurrentIndex(w.link_releases_root.findData(root))
        rows=[dict(path=root+'/'+n+'.flac',root=root,tags={'albumartist':['Artist']},stamp=(1,2)) for n in ('selected','sibling')]
        w.tools_snapshots[root]=rows
        captured=[]
        def worker(operation):
            fake=Mock();fake.start.side_effect=lambda:captured.append(operation(lambda:False,lambda _:None));return fake
        with patch('library_manager.ui.Worker',side_effect=worker),patch('library_manager.ui.BatchMatcher') as batch,patch.object(w,'api',return_value=Mock()),patch.object(self.store,'artists',return_value={'Artist':rows}),patch('library_manager.dj_metadata.DJMetadata'),patch('library_manager.linking.link_recordings',return_value='Checked selected track') as link:
            w.start_linking(recheck=True,target_paths={rows[0]['path']})
            batch.assert_not_called()
            self.assertEqual([r['path'] for r in link.call_args.args[0]],[rows[0]['path']])
            self.assertEqual(len(link.call_args.kwargs['context_rows']),2)
            self.assertFalse(w._link_enabled)
        w._link_worker=None;w.is_linking_active=False;w.demo_mode=True

    def test_sorted_candidate_context_opens_clicked_edition(self):
        w=self.window()
        options=[dict(id='200',track_id='201',artist='Z',album='Z'),dict(id='100',track_id='101',artist='A',album='A')]
        dialog=OnlineAlbumLinkDialog(w,dict(path='/test/file.flac',tags={}),options)
        dialog.show();dialog.table.sortItems(0,Qt.SortOrder.AscendingOrder);app.processEvents()
        with patch('library_manager.ui.QDesktopServices.openUrl',return_value=True) as open_url:
            def trigger():
                for menu in app.topLevelWidgets():
                    if isinstance(menu,QMenu) and menu.isVisible():
                        next(a for a in menu.actions() if a.text()=='Open release online').trigger();menu.close()
            QTimer.singleShot(0,trigger)
            dialog.candidate_menu(dialog.table.visualItemRect(dialog.table.item(0,0)).center())
            self.assertEqual(open_url.call_args.args[0].toString(),'https://tidal.com/album/100')
        dialog.close()

def load_tests(loader,tests,pattern):
    return unittest.TestSuite(ReleaseFollowupTests(name) for name in (
        'test_complete_edition_recovers_omitted_live_label_without_relaxing_pruning',
        'test_context_recheck_uses_only_selected_paths','test_targeted_worker_skips_artist_batch_and_keeps_release_context','test_sorted_candidate_context_opens_clicked_edition'))
