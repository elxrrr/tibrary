import threading,time,json
from unittest.mock import patch,Mock
from PySide6.QtCore import Qt
from test_navigation_and_consolidation import NavigationConsolidationTests,app
from test_release_update import remote
from library_manager.library_workflows import inspect_snapshot

class ScanLayoutTests(NavigationConsolidationTests):
    def test_scan_survives_navigation_and_reuses_completed_snapshot(self):
        w=self.w;root=str(self.root);snapshot=inspect_snapshot(self.root)
        entered=threading.Event();release=threading.Event();threads=[]
        def prepare(*args):
            threads.append(threading.current_thread());entered.set();release.wait(3);return snapshot
        w.tools_snapshots[root]=snapshot;w._dirty_roots.add(root)
        with patch('library_manager.freshness.prepare_library',side_effect=prepare) as scan:
            w.tools_tabs.setCurrentIndex(1);w.invalidate_tools_plan()
            self.assertTrue(entered.wait(2))
            worker=w._preview_worker
            w.nav.setCurrentRow(15);w.tools_tabs.setCurrentIndex(4)
            self.assertFalse(worker.isInterruptionRequested())
            self.assertFalse(w.scan_activity.isHidden())
            release.set();self.finish()
            self.assertEqual(scan.call_count,1)
            self.assertNotEqual(threads[0],threading.main_thread())
            w.nav.setCurrentRow(5);w.invalidate_tools_plan();self.finish()
            self.assertEqual(scan.call_count,1)
            self.assertIn(root,w._prepared_roots)

    def test_partial_selection_and_full_reselection(self):
        w=self.w;r=remote();self.store.enqueue(r);w.set_queue_approval('100',True)
        w.toggle_queue_track_selection('100','11',False)
        self.assertEqual(w.queue_table.item(0,0).checkState(),Qt.CheckState.PartiallyChecked)
        w.set_queue_approval('100',False)
        self.assertEqual(json.loads(self.store.rows('SELECT payload FROM queue')[0]['payload'])['selected_tracks'],[])
        w.set_queue_approval('100',True)
        self.assertIsNone(json.loads(self.store.rows('SELECT payload FROM queue')[0]['payload'])['selected_tracks'])
        w._base_coverage_rows=[dict(release=r,state='Missing release',missing=r['tracks'])]
        w.expanded_releases={'100'};w._render_coverage_table()
        w._missing_checked('100:11',True)
        self.assertEqual(w.coverage_table.model().check_states['100'],Qt.CheckState.PartiallyChecked)
        w._missing_checked('100:12',True)
        self.assertEqual(w.coverage_table.model().check_states['100'],Qt.CheckState.Checked)
        w._missing_checked('100',False)
        self.assertTrue(all(v==Qt.CheckState.Unchecked for v in w.coverage_table.model().check_states.values()))

    def test_refresh_release_list_fetches_and_renders(self):
        w=self.w;w.demo_mode=False;w.market='GB'
        self.store.mapping('Artist','a','confirmed','fixture',True)
        api=Mock();api.artist.return_value=dict(id='a',name='Artist',releases=[remote()])
        def run(operation,completed=None,**kwargs):
            value=operation(lambda:False,lambda _:None)
            if completed:completed(value)
        with patch.object(w,'selected_artist',return_value=('Artist',[])),patch.object(w,'api',return_value=api),patch.object(w,'job',side_effect=run),patch.object(w,'refresh') as refresh:
            w.artist_refresh_releases.click();w.artist_refresh_releases.click()
            self.assertEqual(api.artist.call_count,2);self.assertEqual(refresh.call_count,2)
        self.assertEqual(self.store.cache('a','GB')['releases'][0]['id'],'100')
        w.demo_mode=True

    def test_dashboard_routes_and_audit_columns(self):
        w=self.w
        w.tools_snapshots.clear()
        with patch('library_manager.library_workflows.inspect_snapshot',side_effect=AssertionError('Navigation must not read audio')):
            w.nav.setCurrentRow(2)
        for index,title in ((14,'Link Catalogue'),(15,'Complete Library'),(16,'Settings')):
            w.nav.setCurrentRow(index);self.assertEqual(w.stack.currentIndex(),index)
            self.assertEqual(len([k for k in w.category_cards if k[0]==title]),3)
        mqa=w.mqa_page.table
        self.assertEqual([mqa.horizontalHeaderItem(i).text() for i in range(mqa.columnCount())],['Artist','Release','Local File','Status','Encoded audio','Original rate','Evidence','Target Action'])
        page=w.optimizations_page;page.show_results(self.plans())
        self.assertEqual(page.table.item(0,9).text(),'Remove local duplicates')
        self.assertIn(str(self.root),page.table.item(0,9).toolTip())
        w.provider_download_concurrency.setValue(4)
        self.assertEqual(self.store.preferences('provider')['download_concurrency'],4)
        w.grab().save('/tmp/tibrary-settings-dashboard.png')

def load_tests(loader,tests,pattern):
    import unittest
    return unittest.TestSuite(ScanLayoutTests(name) for name in (
        'test_scan_survives_navigation_and_reuses_completed_snapshot','test_partial_selection_and_full_reselection',
        'test_refresh_release_list_fetches_and_renders','test_dashboard_routes_and_audit_columns'))
