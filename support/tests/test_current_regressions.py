import copy, threading, time, unittest
from unittest.mock import patch
from test_selection_and_identity import SelectionIdentityTests, app
from test_release_update import remote

class CurrentRegressions(SelectionIdentityTests):
    def test_cached_mqa_uses_index_and_invalidates_changed_files(self):
        from library_manager.mqa_audit import cached_audit,SCHEMA
        from unittest.mock import Mock
        store=Mock()
        store.rows.return_value=[dict(path='/music/a.flac',size=123,mtime=456),dict(path='/music/b.flac',size=123,mtime=999)]
        store.preferences.return_value={'files':{path:dict(schema=SCHEMA,stamp=[1,2,123,456,7],result=dict(path=path,status='MQA tags',detected=True,evidence='Encoder')) for path in ('/music/a.flac','/music/b.flac')}}
        rows=cached_audit(store,'/music')
        self.assertEqual([r['status'] for r in rows],['MQA tags','Not checked'])

    def test_no_duplicate_layout_and_catalogue_counts(self):
        from PySide6.QtWidgets import QGroupBox
        w=self.window()
        self.assertNotIn('Folders & filenames',[b.title() for b in w.findChildren(QGroupBox)])
        w.update_category_cards(dict(artists={'One':[],'Two':[]},mappings={'One':{'status':'confirmed'}},link_statistics={'linked_releases':3,'release_count':4}))
        self.assertIn('1 / 2 artists linked',w.category_cards['Link Catalogue',0].text())
        self.assertIn('3 / 4 releases linked',w.category_cards['Link Catalogue',1].text())

    def test_settings_save_does_not_rebuild_library(self):
        w=self.window()
        with patch.object(w,'invalidate_tools_plan') as rebuild:
            w.streaming_layout_template.editingFinished.emit()
            rebuild.assert_not_called()
        self.assertIsNone(w._prepared_mode)
        self.assertTrue(w.statusBar().isHidden())

    def test_job_disables_scan_without_showing_status_bar(self):
        w=self.window();finished=threading.Event()
        def work(cancel,progress):
            finished.wait(2)
        w.job(work,label='Test background work',local=True)
        try:
            self.assertFalse(w.scan_releases_button.isEnabled())
            self.assertTrue(w.statusBar().isHidden())
        finally:finished.set()
        deadline=time.monotonic()+3
        while w.worker and time.monotonic()<deadline:app.processEvents();time.sleep(.01)
        self.assertIsNone(w.worker)
        self.assertTrue(w.scan_releases_button.isEnabled())
        self.assertTrue(w.statusBar().isHidden())

    def test_disclosure_updates_current_rows_after_refresh(self):
        w=self.window();w.demo_mode=False
        release=remote();release['tracks']=[];release['tracks_loaded']=False
        w._base_coverage_rows=[dict(release=release,state='Missing release',missing=[])]
        w._render_coverage_table()
        with patch.object(w,'job') as job:
            w.expand_release_id(str(release['id']))
            callback=job.call_args.args[1]
        w._base_coverage_rows=copy.deepcopy(w._base_coverage_rows)
        callback(remote())
        self.assertTrue(w._base_coverage_rows[0]['release']['tracks'])
        self.assertIn(str(release['id']),w.expanded_releases)
        self.assertGreater(w.coverage_table.rowCount(),1)

def load_tests(loader,tests,pattern):
    return loader.loadTestsFromNames([f'{__name__}.CurrentRegressions.{name}' for name in CurrentRegressions.__dict__ if name.startswith('test_')])
