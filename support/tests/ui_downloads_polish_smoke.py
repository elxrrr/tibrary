import os
import unittest
from pathlib import Path
from PySide6.QtWidgets import QApplication
from library_manager.core import Store
from library_manager.ui import Window, TidalFavouritesDialog

app = QApplication.instance() or QApplication([])

class TestDownloadsAndPolishUI(unittest.TestCase):
    def setUp(self):
        self.db_path = Path('/tmp/test_downloads_polish.db')
        if self.db_path.exists(): self.db_path.unlink()
        self.store = Store(self.db_path)
        self.window = Window(self.store, demo_mode=True)

    def tearDown(self):
        self.window.close()
        if self.db_path.exists(): self.db_path.unlink()

    def test_downloads_settings_tab(self):
        # Settings tab index 2 is Downloads
        tab_text = self.window.settings_tabs.tabText(2)
        self.assertEqual(tab_text, 'Downloads')
        self.assertTrue(hasattr(self.window, 'download_folder'))
        self.assertTrue(hasattr(self.window, 'download_quality'))
        self.assertTrue(hasattr(self.window, 'download_segments'))
        self.assertTrue(hasattr(self.window, 'download_replaygain'))

    def test_queue_page_polish(self):
        # Destination caption exists and choose folder button is gone from queue page
        self.assertTrue(hasattr(self.window, 'queue_dest_note'))
        self.assertIn('Downloads save to:', self.window.queue_dest_note.text())
        # Columns
        expected_cols = ['Approve', 'Album Artist', 'Release', 'Selection', 'Date', 'Type', 'Tracks', 'Availability']
        for i, col in enumerate(expected_cols):
            self.assertEqual(self.window.queue_table.horizontalHeaderItem(i).text(), col)

    def test_tools_page_details_toggle_and_duplicate_tracks(self):
        # Detail box is hidden by default
        self.assertTrue(self.window.tools_detail.isHidden())
        self.assertEqual(self.window.tools_details_btn.text(), 'Show details')
        # Toggle button
        self.window.tools_details_btn.setChecked(True)
        self.assertFalse(self.window.tools_detail.isHidden())
        self.assertEqual(self.window.tools_details_btn.text(), 'Hide details')
        self.window.tools_details_btn.setChecked(False)
        self.assertTrue(self.window.tools_detail.isHidden())
        # Duplicate tracks checkbox is unchecked by default
        self.assertTrue(hasattr(self.window, 'tools_duplicate_tracks'))
        self.assertFalse(self.window.tools_duplicate_tracks.isChecked())

    def test_table_density(self):
        self.assertTrue(hasattr(self.window, 'table_density'))
        self.window.change_table_density('Compact (11 pt)')
        self.assertEqual(self.window.queue_table.verticalHeader().defaultSectionSize(), 26)
        self.window.change_table_density('Comfortable (13 pt)')
        self.assertEqual(self.window.queue_table.verticalHeader().defaultSectionSize(), 32)
        self.window.change_table_density('Default (12 pt)')
        self.assertEqual(self.window.queue_table.verticalHeader().defaultSectionSize(), 28)

    def test_online_status_badge(self):
        self.assertTrue(hasattr(self.window, 'online_status_label'))
        self.assertTrue('Online API' in self.window.online_status_label.text() or 'TIDAL API' in self.window.online_status_label.text())

    def test_favourites_dialog_breakdown(self):
        mock_favs = [
            dict(id='900001', name='North Assembly'),
            dict(id='900099', name='Solar Fields'),
            dict(id='900100', name='Carbon Based Lifeforms')
        ]
        dlg = TidalFavouritesDialog(self.window, mock_favs)
        self.assertEqual(dlg.windowTitle(), 'TIDAL Favourites Comparison')
        tabs = dlg.findChild(type(self.window.settings_tabs))
        self.assertEqual(tabs.count(), 3)
        self.assertIn('Missing locally', tabs.tabText(0))
        self.assertIn('In local', tabs.tabText(1))
        self.assertIn('Local only', tabs.tabText(2))
        dlg.close()

if __name__ == '__main__':
    unittest.main()
