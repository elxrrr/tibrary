import copy,tempfile,time,unittest
from pathlib import Path
from unittest.mock import patch
from PySide6.QtWidgets import QApplication,QMessageBox,QMenu
from PySide6.QtCore import QTimer
from library_manager.core import Store,scan
from library_manager.ui import Window,HealthCard
from library_manager.optimizations import find_optimizations,save_optimization_results,load_optimization_results
from test_release_update import audio_file
app=QApplication.instance() or QApplication([])

class NavigationConsolidationTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name).resolve()/'music'
        self.store=Store(Path(self.tmp.name)/'db')
        audio_file(self.root/'Artist/Single/1.flac',1,1,'Song 1','TEST1',album='Single')
        audio_file(self.root/'Artist/Album/1.flac',1,2,'Song 1','TEST1',album='Album')
        audio_file(self.root/'Artist/Album/2.flac',2,2,'Song 2','TEST2',album='Album')
        scan(self.store,self.root);self.w=Window(self.store,demo_mode=True)
        self.w._bg_linking_timer.stop();self.w.resize(1400,1000);self.w.show();app.processEvents()
    def tearDown(self):self.w.close();app.processEvents();self.tmp.cleanup()
    def finish(self):
        end=time.monotonic()+5
        while time.monotonic()<end:
            app.processEvents();time.sleep(.005)
            if not self.w.worker and not self.w._preview_worker:app.processEvents();return
        self.fail('Operation did not finish')
    def plans(self):return find_optimizations(self.store,'GB',self.root,scope='local')
    def prepare_page(self):
        w=self.w;w.market='GB';w.nav.setCurrentRow(11);app.processEvents()
        page=w.optimizations_page;page.result_root=str(self.root);page.show_results(self.plans());page.table.selectRow(0)
        w.demo_mode=False;return page
    def test_merge_button_and_context_action_reach_confirmation(self):
        page=self.prepare_page()
        with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.No) as confirm:
            page.action.click();self.finish();self.assertEqual(confirm.call_count,1)
            def trigger():
                for menu in app.topLevelWidgets():
                    if isinstance(menu,QMenu) and menu.isVisible():
                        next(a for a in menu.actions() if a.text()=='Remove local duplicates…').trigger();menu.close()
            QTimer.singleShot(0,trigger)
            page.context_menu(page.table.visualRect(page.table.model().index(0,0)).center())
            self.finish();self.assertEqual(confirm.call_count,2)
        self.assertTrue((self.root/'Artist/Single/1.flac').exists())
    def test_failure_is_visible_on_consolidation_page(self):
        page=self.prepare_page()
        with patch('library_manager.optimizations.validate_consolidation',side_effect=ValueError('Replacement changed; refresh comparison')):
            page.action.click();self.finish()
        self.assertIn('Replacement changed',page.status.text())
    def test_saved_opportunities_reload_and_changed_files_invalidate(self):
        plans=self.plans();save_optimization_results(self.store,self.root,'GB','local',plans)
        self.assertEqual(len(load_optimization_results(self.store,self.root,'GB','local')),1)
        self.w.market='GB';self.w.nav.setCurrentRow(11);app.processEvents()
        self.assertEqual(len(self.w.optimizations_page.rows),1)
        with self.store.connect() as db:db.execute('UPDATE local_files SET mtime=mtime+1')
        self.assertIsNone(load_optimization_results(self.store,self.root,'GB','local'))
    def test_compact_headers_visible_empty_tables_and_health_cards(self):
        w=self.w;w.nav.setCurrentRow(5)
        from library_manager.library_workflows import inspect_snapshot
        w.tools_plan=inspect_snapshot(self.root);w.tools_snapshots[str(self.root)]=w.tools_plan
        w._update_all_tool_counts(w.tools_plan)
        for tab in (2,3):
            w.tools_tabs.setCurrentIndex(tab);self.finish()
            self.assertFalse(w.tools_workspace.isHidden())
            self.assertLess(w.tools_tabs.height(),230)
            self.assertGreater(w.tools_table.height(),400)
        w.tools_tabs.setCurrentIndex(0);self.finish()
        self.assertTrue(all(isinstance(c,HealthCard) for c in w.tools_summary_buttons.values()))
        self.assertIn('files',w.tools_summary_buttons['metadata'].text())
        w.grab().save('/tmp/tibrary-health-followup.png')
        w.tools_tabs.setCurrentIndex(2);self.finish();w.grab().save('/tmp/tibrary-metadata-followup.png')
    def test_navigation_reuses_prepared_views_without_file_reads(self):
        from library_manager.library_workflows import inspect_snapshot
        w=self.w;root=str(self.root)
        w.tools_root.setCurrentIndex(w.tools_root.findData(root))
        w.tools_snapshots[root]=inspect_snapshot(self.root);w.tools_plan=w.tools_snapshots[root]
        w.tools_tabs.setCurrentIndex(2);self.finish();w.tools_tabs.setCurrentIndex(3);self.finish()
        with patch('library_manager.library_workflows.workflow_plan',side_effect=AssertionError('Preview rebuilt')), patch('library_manager.freshness.prepare_library',side_effect=AssertionError('Files rescanned')):
            w.tools_tabs.setCurrentIndex(2);self.finish()
