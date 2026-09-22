import copy,json,time
from unittest.mock import Mock,patch
from PySide6.QtCore import Qt
from PySide6.QtTest import QTest
from test_navigation_and_consolidation import NavigationConsolidationTests,app
from test_selection_and_identity import SelectionIdentityTests
from test_release_update import remote,audio_file
from library_manager.core import scan
from library_manager.optimizations import check_candidate_releases,find_optimizations,save_optimization_results

class QueueInteractionTests(SelectionIdentityTests):
    def test_real_disclosure_double_click_and_native_approval(self):
        w=self.window();self.store.enqueue(remote());w.refresh_queue()
        w.nav.setCurrentRow(4);w.resize(1200,850);w.show();app.processEvents()
        with patch.object(w,'refresh_overview_tables') as dashboard:
            for count in (3,1,3,1):
                rect=w.queue_table.visualItemRect(w.queue_table.item(0,1))
                QTest.mouseClick(w.queue_table.viewport(),Qt.MouseButton.LeftButton,pos=rect.center())
                QTest.mouseDClick(w.queue_table.viewport(),Qt.MouseButton.LeftButton,pos=rect.center())
                app.processEvents()
                self.assertEqual(w.queue_table.rowCount(),count)
            dashboard.assert_not_called()
        self.assertIsNone(w.queue_table.cellWidget(0,0))
        w.queue_table.item(0,0).setCheckState(Qt.CheckState.Checked);app.processEvents()
        self.assertTrue(self.store.rows('SELECT approved FROM queue')[0]['approved'])

    def test_missing_native_checkbox_keeps_sorted_expanded_rows(self):
        from PySide6.QtWidgets import QStyleOptionViewItem,QStyle
        w=self.window();w.nav.setCurrentRow(3);w.resize(1200,850);w.show();app.processEvents()
        r=remote();w._base_coverage_rows=[dict(release=r,state='Missing release',missing=r['tracks'])]
        w.expanded_releases={'100'};w._render_coverage_table()
        table=w.coverage_table;table.sortByColumn(5,Qt.SortOrder.DescendingOrder)
        index=table.model().index(0,9)
        option=QStyleOptionViewItem();option.initFrom(table);option.rect=table.visualRect(index)
        table.itemDelegate().initStyleOption(option,index)
        rect=table.style().subElementRect(QStyle.SubElement.SE_ItemViewItemCheckIndicator,option,table)
        QTest.mouseClick(table.viewport(),Qt.MouseButton.LeftButton,pos=rect.center());app.processEvents()
        self.assertIsNone(w._missing_selection['100'])
        self.assertEqual(table.model().sort_column,5)
        self.assertEqual(table.model().keys,['100','100:11','100:12'])
        self.assertIn('100',w.expanded_releases)

class OptimizationFollowupTests(NavigationConsolidationTests):
    def test_saved_results_survive_invalidations_and_page_navigation(self):
        self.w.market='GB';self.w.nav.setCurrentRow(11);app.processEvents()
        page=self.w.optimizations_page;plans=self.plans()
        save_optimization_results(self.store,self.root,'GB','local',plans)
        page.result_root=str(self.root);page.show_results(plans)
        self.w._library_content_changed(str(self.root),refresh=False)
        self.assertEqual(len(page.rows),1)
        with self.store.connect() as db:db.execute('UPDATE local_files SET mtime=mtime+1')
        page.root_changed();self.assertEqual(len(page.rows),1)
        self.w.nav.setCurrentRow(0);self.w.nav.setCurrentRow(11);app.processEvents()
        self.assertEqual(len(page.rows),1)

    def test_missing_performer_evidence_is_fetched_and_reused(self):
        # Three-track local release and a four-track rolling edition, as Unbound.
        for n in range(1,4):audio_file(self.root/'Artist/Unbound'/f'{n}.flac',n,3,f'Song {n}',f'TEST{n}',album='Unbound')
        scan(self.store,self.root);self.store.mapping('Artist','a','confirmed','fixture',True)
        release=remote(4,'400');release['title']='Unbound'
        release['optimization_credits_checked']=True  # Legacy marker lacked track credits.
        self.store.save_catalogue('a','GB',dict(releases=[release]))
        self.assertFalse(any(p['folder'].endswith('Unbound') for p in find_optimizations(self.store,'GB',self.root,scope='remote')))
        api=Mock();api.album_tag_details.return_value=copy.deepcopy(release)
        api.track_tag_details.side_effect=lambda t:dict(t,artists=['Artist','Guest'])
        self.assertEqual(check_candidate_releases(self.store,'GB',self.root,api),1)
        plans=find_optimizations(self.store,'GB',self.root,scope='remote')
        self.assertEqual(len([p for p in plans if p['folder'].endswith('Unbound')]),1)
        self.assertEqual(api.track_tag_details.call_count,4)
        self.assertEqual(check_candidate_releases(self.store,'GB',self.root,api),0)
        self.assertEqual(api.track_tag_details.call_count,4)

def load_tests(loader,tests,pattern):
    import unittest
    return unittest.TestSuite([
        QueueInteractionTests('test_real_disclosure_double_click_and_native_approval'),
        QueueInteractionTests('test_missing_native_checkbox_keeps_sorted_expanded_rows'),
        OptimizationFollowupTests('test_saved_results_survive_invalidations_and_page_navigation'),
        OptimizationFollowupTests('test_missing_performer_evidence_is_fetched_and_reused'),
    ])
