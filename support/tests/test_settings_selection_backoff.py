import copy,unittest
from unittest.mock import patch
from PySide6.QtCore import Qt
from test_selection_and_identity import SelectionIdentityTests,app
from test_release_update import remote
from library_manager.tidal import RequestPacer,CatalogueError
from library_manager.client_settings import DEFAULTS

class SettingsSelectionTests(SelectionIdentityTests):
    def test_independent_resets_and_combined_path_area(self):
        w=self.window();w.resize(1650,1050);w.show();w.nav.setCurrentRow(6);w.settings_tabs.setCurrentIndex(2);app.processEvents()
        w.provider_download_concurrency.setValue(1);w.provider_rate_interval.setValue(900)
        w.provider_reset.click()
        self.assertEqual(w.provider_download_concurrency.value(),1)
        self.assertEqual(w.provider_rate_interval.value(),DEFAULTS['request_interval_ms'])
        w.provider_rate_interval.setValue(800);w.download_engine_reset.click()
        self.assertEqual(w.provider_rate_interval.value(),800)
        self.assertEqual(w.provider_download_concurrency.value(),DEFAULTS['download_concurrency'])
        self.assertEqual(w.download_quality.currentData(),'Lossless')
        self.assertEqual(w.streaming_layout_template.parentWidget(),w.download_folder.parentWidget())
        w.grab().save('/tmp/tibrary-download-settings-polished.png')

    def test_settings_cards_report_per_library_and_connection_health(self):
        w=self.window();data=dict(roots=[{'root':'/Music'}],library_link_counts=[dict(root='/Music',track_count=12,linked_tracks=9)])
        w._connection_metrics={key:dict(ok=True) for key in ('search','account','credentials','download')}
        w.update_category_cards(data)
        self.assertIn('9 / 12 tracks linked',w.category_cards['Settings',0].description_label.text())
        self.assertIn('All connected',w.category_cards['Settings',1].text())
        w._connection_metrics['download']['ok']=False;w.update_category_cards(data)
        self.assertIn('Needs attention',w.category_cards['Settings',1].text())
        self.assertIn('Downloads',w.category_cards['Settings',1].description_label.text())

    def test_every_visible_row_draws_its_own_checkbox(self):
        w=self.window();w.resize(1400,900);w.show();w.nav.setCurrentRow(3);app.processEvents()
        rows=[]
        for n in range(4):
            release=remote(2,str(100+n));rows.append(dict(release=release,state='Missing release',missing=release['tracks']))
        w._base_coverage_rows=rows;w.expanded_releases={'100'};w._render_coverage_table();app.processEvents()
        t=w.coverage_table;t.horizontalScrollBar().setValue(0);app.processEvents()
        image=t.viewport().grab().toImage();ratio=image.devicePixelRatio()
        for row in range(t.rowCount()):
            rect=t.visualRect(t.model().index(row,9));x,y=rect.center().x(),rect.center().y()
            self.assertNotEqual(image.pixelColor(int((x-8)*ratio),int(y*ratio)),image.pixelColor(int((x-14)*ratio),int(y*ratio)),row)
        from PySide6.QtTest import QTest
        row=t.model().keys.index('101')
        QTest.mouseClick(t.viewport(),Qt.MouseButton.LeftButton,pos=t.visualRect(t.model().index(row,9)).center());app.processEvents()
        self.assertIsNone(w._missing_selection['101']);self.assertNotIn('102',w._missing_selection)
        w.change_theme('Dark');app.processEvents()
        w.grab().save('/tmp/tibrary-checkboxes-polished.png')
        for item in rows:self.store.enqueue(item['release'])
        w.refresh_queue();w.nav.setCurrentRow(4);app.processEvents()
        QTest.mouseClick(w.queue_table.viewport(),Qt.MouseButton.LeftButton,pos=w.queue_table.visualItemRect(w.queue_table.item(2,0)).center());app.processEvents()
        self.assertEqual(sum(r['approved'] for r in self.store.rows('SELECT approved FROM queue')),1)


class BackoffTests(unittest.TestCase):
    def test_shared_backoff_and_budget_across_requests(self):
        clock=[100.0]
        with patch('library_manager.tidal.time.monotonic',lambda:clock[0]),patch('library_manager.tidal.time.sleep',lambda d:clock.__setitem__(0,clock[0]+d)):
            p=RequestPacer(.35)
            self.assertEqual(p.record_429(4),30)
            p.wait(lambda:False);self.assertGreaterEqual(clock[0],130)
            p.record_429(4);p.wait(lambda:False)
            with self.assertRaises(CatalogueError) as error:p.record_429(4)
            self.assertEqual(error.exception.status,429);self.assertTrue(error.exception.batch_fatal)
            with self.assertRaises(CatalogueError):p.wait(lambda:True)

def load_tests(loader,tests,pattern):
    return unittest.TestSuite([SettingsSelectionTests('test_independent_resets_and_combined_path_area'),SettingsSelectionTests('test_settings_cards_report_per_library_and_connection_health'),SettingsSelectionTests('test_every_visible_row_draws_its_own_checkbox'),BackoffTests('test_shared_backoff_and_budget_across_requests')])
