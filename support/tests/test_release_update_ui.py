import tempfile,copy,unittest
from pathlib import Path
from unittest.mock import patch
from PySide6.QtWidgets import QApplication
from PySide6.QtCore import Qt,QPoint,QTimer
from PySide6.QtWidgets import QMenu
from library_manager.core import Store,scan
from library_manager.ui import Window,OnlineAlbumLinkDialog
from library_manager.maintenance import inspect_file
from test_release_update import audio_file
app=QApplication.instance() or QApplication([])

class ReleaseUpdateUITests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name).resolve()/'music'
        self.path=audio_file(self.root/'Artist/Album/song.flac');self.store=Store(Path(self.tmp.name)/'db');scan(self.store,self.root)
        self.window=Window(self.store,demo_mode=True)
    def tearDown(self):
        self.window.close();self.tmp.cleanup()
    def test_attention_updates_immediately_and_keeps_edition_options_scoped(self):
        w=self.window;row=inspect_file(self.path,self.root)
        row['catalogue_options']=[dict(id='1',track_id='11',artist='Artist',album='Album',changes={},recording_verified=True)]
        row['catalogue_note']='Needs choice';w.tools_snapshots[str(self.root)]=[row]
        w.link_releases_root.addItem(str(self.root),str(self.root));w.link_releases_root.setCurrentIndex(w.link_releases_root.findData(str(self.root)))
        w.render_link_releases();w.link_filter.setCurrentText('Needs attention')
        self.assertFalse(w.link_table.isRowHidden(0))
        updated=copy.deepcopy(row);updated.update(catalogue_choice=row['catalogue_options'][0],linked_ids=dict(album_id='1',track_id='11'))
        from library_manager.linking import save_result
        self.assertTrue(save_result(self.store,w.market,updated,manual=True))
        w.link_row_saved(updated)
        self.assertTrue(w.link_table.isRowHidden(0));w.filter_link_table();self.assertTrue(w.link_table.isRowHidden(0))
    def test_sorted_edition_dialog_preserves_selected_id(self):
        row=inspect_file(self.path,self.root)
        options=[dict(id='1',artist='Zulu',album='A',track_id='11',changes={}),dict(id='2',artist='Alpha',album='B',track_id='21',changes={})]
        dialog=OnlineAlbumLinkDialog(self.window,row,options)
        dialog.table.sortItems(0,Qt.SortOrder.AscendingOrder);dialog.table.selectRow(0);dialog._on_link_clicked()
        self.assertEqual(dialog.chosen_option['id'],'2')
    def test_new_navigation_and_mqa_filters(self):
        w=self.window;w.nav.setCurrentRow(10);self.assertEqual(w.stack.currentWidget(),w.mqa_page)
        w.mqa_page.show_results([dict(path='a',status='MQA signal',detected=True,evidence='test'),dict(path='b',status='No signal found',detected=False,evidence='test')])
        self.assertEqual(sum(w.mqa_page.table.isRowHidden(i) for i in range(2)),1)
        w.mqa_page.filter.setCurrentText('All files');self.assertFalse(any(w.mqa_page.table.isRowHidden(i) for i in range(2)))
        w.nav.setCurrentRow(11);self.assertEqual(w.stack.currentWidget(),w.optimizations_page)
        self.assertIn('Prepare Library',w.nav.currentItem().parent().text())
        w.nav.setCurrentRow(12);self.assertEqual(w.stack.currentWidget(),w.online_optimizations_page)
        self.assertIn('Fix Library',w.nav.currentItem().parent().text())
        self.assertEqual(w.download_quality.minimumHeight(),w.download_folder.minimumHeight())

    def test_virtual_coverage_context_menu_uses_model_index_and_guards_empty_space(self):
        w=self.window
        release=dict(id='1',title='Release',artist='Artist',date='2024',type='SINGLE',track_count=1,available=True)
        item=dict(release=release,state='Missing release',local_tracks=[str(self.path)],recommendation=dict(badge='Potential',evidence=[]))
        w._base_coverage_rows=[item];w._render_coverage_table();w.coverage_table.resize(800,300);w.coverage_table.show();app.processEvents()
        w._coverage_table_context_menu(QPoint(w.coverage_table.viewport().width()+100,w.coverage_table.viewport().height()+100))
        pos=w.coverage_table.visualRect(w.coverage_table.model().index(0,0)).center()
        QTimer.singleShot(0,lambda:[menu.close() for menu in app.topLevelWidgets() if isinstance(menu,QMenu)])
        w._coverage_table_context_menu(pos)
        self.assertEqual(w.coverage_table.currentIndex().row(),0)

    def test_streaming_settings_persist_and_apply_without_restart(self):
        w=self.window
        w.provider_rate_interval.setValue(725)
        w.provider_timeout.setValue(22)
        w.provider_download_concurrency.setValue(2)
        app.processEvents()
        saved=self.store.preferences('provider')
        self.assertEqual(saved['request_interval_ms'],725);self.assertEqual(saved['request_timeout_sec'],22)
        self.assertEqual(saved['download_concurrency'],2);self.assertAlmostEqual(w.request_pacer.base_interval,.725)

    def test_provider_reset_and_downloaded_overview(self):
        from library_manager.client_settings import DEFAULTS
        w=self.window
        w.provider_timeout.setValue(22);w.provider_reset.click()
        self.assertEqual(self.store.preferences('provider'),DEFAULTS)
        self.store.enqueue(dict(id='ignored',title='Not downloaded',artist='A',available=True))
        with self.store.connect() as db:db.execute("UPDATE queue SET decision='ignored'")
        w.refresh_overview_tables()
        self.assertEqual(w.overview_downloaded.rowCount(),0)

    def test_tag_action_defaults_to_actual_affected_files(self):
        w=self.window;row=inspect_file(self.path,self.root)
        w.tools_snapshots[str(self.root)]=[row]
        w.tools_tabs.setCurrentIndex(1)
        self.assertEqual(w.tools_filter.currentText(),'Proposed changes')
        w.tools_filter.setCurrentText('All files')
        w.tools_discs.click()
        self.assertEqual(w.tools_filter.currentText(),'Proposed changes')
        w.tools_tabs.setCurrentIndex(0)
        self.assertTrue(w.tools_workspace.isHidden())

    def test_link_filters_include_unchecked_and_exclude_saved_links(self):
        from library_manager.ui import fill
        w=self.window
        records=[dict(path='unchecked'),dict(path='linked',linked_ids={'album_id':'1','track_id':'11'}),
                 dict(path='choice',catalogue_options=[{'id':'2'}]),dict(path='ignored')]
        self.store.ignore_local_file('ignored')
        w.link_plan=records
        fill(w.link_table,[(r['path'],'','', '', '') for r in records],keys=[r['path'] for r in records])
        expected={'Needs attention':{'unchecked','choice'},'Unlinked tracks':{'unchecked','choice'},
                  'Linked tracks':{'linked'},'Too many editions / Needs choice':{'choice'},
                  'Ignored files':{'ignored'},'All files':{'unchecked','linked','choice','ignored'}}
        selector=patch('library_manager.link_statistics.link_statistics',return_value={'active':{'linked':records[1]['linked_ids']}})
        selector.start();self.addCleanup(selector.stop)
        for name,visible in expected.items():
            w.link_filter.setCurrentText(name);w.filter_link_table()
            self.assertEqual({r['path'] for i,r in enumerate(records) if not w.link_table.isRowHidden(i)},visible,name)
        w.link_filter.setCurrentText('All files');w.link_search.setText('unchecked')
        self.assertEqual([i for i in range(4) if not w.link_table.isRowHidden(i)],[0])
