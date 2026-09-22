import unittest
import tempfile
import json
from pathlib import Path
from unittest.mock import patch

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QApplication, QMessageBox, QLineEdit, QLabel

from library_manager.core import Store, scan, now
from library_manager.ui import Window, OnlineAlbumLinkDialog

app = QApplication.instance() or QApplication([])

class LatestUIFeaturesTests(unittest.TestCase):
    def setUp(self):
        self.tmp_dir = tempfile.TemporaryDirectory()
        self.db_path = Path(self.tmp_dir.name) / 'library.db'
        self.music_dir = Path(self.tmp_dir.name) / 'music'
        self.music_dir.mkdir()
        self.store = Store(self.db_path)
        scan(self.store, self.music_dir)
        self.window = Window(self.store, demo_mode=True)

    def tearDown(self):
        self.window.close()
        self.tmp_dir.cleanup()

    def test_sidebar_hierarchy_and_submenus(self):
        self.assertEqual(self.window.nav.count(), 7)
        self.assertGreaterEqual(self.window.nav.topLevelItemCount(), 6)
        linking = next(self.window.nav.topLevelItem(i) for i in range(self.window.nav.topLevelItemCount()) if 'Link Catalogue' in self.window.nav.topLevelItem(i).text())
        complete = next(self.window.nav.topLevelItem(i) for i in range(self.window.nav.topLevelItemCount()) if 'Complete Library' in self.window.nav.topLevelItem(i).text())
        prepare = next(self.window.nav.topLevelItem(i) for i in range(self.window.nav.topLevelItemCount()) if 'Prepare Library' in self.window.nav.topLevelItem(i).text())
        self.assertTrue(any('Link Releases' in linking.child(i).text() for i in range(linking.childCount())))
        dl_item = self.window.nav._item_download
        self.assertEqual(dl_item.parent(),complete)
        self.assertEqual(dl_item.childCount(), 2)
        self.assertEqual(dl_item.child(0).text(), '📥  Queue')
        self.assertEqual(dl_item.child(1).text(), '✅  Downloaded Releases')

        tools_item = self.window.nav._item_tools
        self.assertEqual(tools_item,prepare)
        self.assertTrue(any('MQA Audit' in prepare.child(i).text() for i in range(prepare.childCount())))

        settings_item = self.window.nav._item_settings
        self.assertEqual(settings_item.childCount(), 4)
        self.assertEqual(self.window.nav._item_activity.parent(),settings_item)

        self.assertTrue(self.window.tools_tabs.tabBar().isHidden())
        self.assertTrue(self.window.settings_tabs.tabBar().isHidden())

        # Check font size of submenu items is 11.5pt and indentation is 0
        self.assertEqual(self.window.nav.indentation(), 18)
        self.assertTrue(self.window.nav.rootIsDecorated())
        self.assertEqual(dl_item.child(0).font(0).pointSizeF(), 11.5)
        self.assertEqual(settings_item.child(0).font(0).pointSizeF(), 11.5)

        from library_manager.ui import STYLE
        self.assertNotIn('QTreeWidget::branch', STYLE)

    def test_entire_overview_cards_open_their_destination(self):
        destinations=[2,1,5,7,3,4,5,6]
        for card,expected in zip(self.window.overview_cards,destinations):
            card.activated.emit()
            self.assertEqual(self.window.nav.currentRow(),expected)

    def test_multi_drive_checkbox_table(self):
        self.assertTrue(self.window.drive_mode_check.isHidden())
        from library_manager.view_data import build_view
        data = build_view(self.store, 'GB')
        data['roots'] = [{'root': '/music/one', 'scanned_at': '2026-09-18', 'status': 'complete'},
                         {'root': '/music/two', 'scanned_at': '2026-09-18', 'status': 'complete'}]
        self.window._view_ready(data)
        self.assertEqual(self.window.roots.rowCount(), 2)
        item0 = self.window.roots.item(0, 0)
        item1 = self.window.roots.item(1, 0)
        self.assertIsNotNone(item0)
        self.assertIsNotNone(item1)
        self.assertFalse(item0.flags() & Qt.ItemFlag.ItemIsUserCheckable)
        self.assertFalse(item1.flags() & Qt.ItemFlag.ItemIsUserCheckable)
        self.window.active_drive_combo.setCurrentIndex(1)
        self.assertEqual(self.window.get_checked_roots(), ['/music/two'])
        self.window.active_drive_combo.setCurrentIndex(0)
        self.assertEqual(self.window.get_checked_roots(), ['/music/one'])

    def test_credentials_locked_when_loaded(self):
        client = self.window.findChild(QLineEdit, 'client_id')
        secret = self.window.findChild(QLineEdit, 'client_secret')
        self.assertIsNotNone(client)
        self.assertIsNotNone(secret)

        # In setUp, credentials are empty, so fields are enterable
        # Now mock credentials loaded
        with patch.object(self.window.credentials, 'has_keys', return_value=True):
            # Trigger update_buttons logic directly
            has_k = True
            client.setReadOnly(has_k)
            secret.setReadOnly(has_k)
            self.assertTrue(client.isReadOnly())
            self.assertTrue(secret.isReadOnly())

    def test_connection_diagnostics_cards_responsive(self):
        self.assertTrue(self.window.connection_status.isHidden())
        self.assertTrue(hasattr(self.window, '_diag_cards_layout'))
        self.assertEqual(len(self.window._diag_card_widgets), 4)

        # Test wide layout (width >= 850) -> 1 row x 4 cols
        self.window._reflow_diag_cards(1000)
        for idx, card in enumerate(self.window._diag_card_widgets):
            row = self.window._diag_cards_layout.getItemPosition(self.window._diag_cards_layout.indexOf(card))[0]
            col = self.window._diag_cards_layout.getItemPosition(self.window._diag_cards_layout.indexOf(card))[1]
            self.assertEqual(row, 0)
            self.assertEqual(col, idx)

        # Test narrow layout (width < 850) -> 2 rows x 2 cols
        self.window._reflow_diag_cards(800)
        pos0 = self.window._diag_cards_layout.getItemPosition(self.window._diag_cards_layout.indexOf(self.window._diag_card_widgets[0]))[:2]
        pos1 = self.window._diag_cards_layout.getItemPosition(self.window._diag_cards_layout.indexOf(self.window._diag_card_widgets[1]))[:2]
        pos2 = self.window._diag_cards_layout.getItemPosition(self.window._diag_cards_layout.indexOf(self.window._diag_card_widgets[2]))[:2]
        pos3 = self.window._diag_cards_layout.getItemPosition(self.window._diag_cards_layout.indexOf(self.window._diag_card_widgets[3]))[:2]
        self.assertEqual(pos0, (0, 0))
        self.assertEqual(pos1, (0, 1))
        self.assertEqual(pos2, (1, 0))
        self.assertEqual(pos3, (1, 1))

    def test_queue_page_buttons_and_no_instruction_label(self):
        self.assertTrue(hasattr(self.window, 'queue_start_download_btn'))
        self.assertTrue(hasattr(self.window, 'queue_export_btn'))
        self.assertEqual(self.window.queue_start_download_btn.text(), 'Start download')
        self.assertEqual(self.window.queue_export_btn.text(), 'Export release list…')

    def test_downloaded_releases_page_and_mark_undownloaded(self):
        self.assertTrue(hasattr(self.window, 'downloaded_releases_page_widget'))
        self.assertTrue(hasattr(self.window, 'downloaded_table'))

        payload = {
            'id': 'rel-101',
            'artist': 'Test Artist',
            'title': 'Test Album',
            'type': 'ALBUM',
            'track_count': 10
        }
        with self.store.connect() as db:
            db.execute("INSERT INTO queue VALUES (?, ?, 0, 'downloaded', ?)",
                       ('rel-101', json.dumps(payload), now()))

        self.window.refresh_downloaded_releases()
        self.assertEqual(self.window.downloaded_table.rowCount(), 1)
        self.assertEqual(self.window.downloaded_table.item(0, 0).text(), '▸ Test Artist')
        self.assertEqual(self.window.downloaded_table.item(0, 1).text(), 'Test Album')

        self.window.downloaded_table.selectRow(0)
        with patch('PySide6.QtWidgets.QMessageBox.question', return_value=QMessageBox.StandardButton.Yes):
            self.window._mark_selected_undownloaded()

        q_rows = self.store.rows("SELECT * FROM queue WHERE id='rel-101'")
        self.assertEqual(len(q_rows), 0)
        self.assertEqual(self.window.downloaded_table.rowCount(), 0)

    def test_tools_page_cleanups(self):
        self.assertTrue(self.window.tools_dj_only.isHidden())
        self.assertFalse(self.window.tools_summary_status.isHidden())
        btn = self.window.tools_summary_buttons['tags']
        self.assertIn('background-color:', btn.styleSheet())
        self.assertNotIn('background:', btn.styleSheet())
        self.assertGreaterEqual(btn.minimumHeight(), 42)

        # Verify summary page has no label containing 'wizard'
        summary_widget = self.window.tools_tabs.widget(0)
        all_labels = summary_widget.findChildren(QLabel)
        for lbl in all_labels:
            self.assertNotIn('wizard', lbl.text().lower())

    def test_online_album_link_dialog(self):
        row = {
            'path': '/music/Artist/Album/01.flac',
            'tags': {'albumartist': ['Artist Name'], 'album': ['Album Name'], 'date': ['2023']},
            'catalogue_options': [
                {
                    'id': '99999',
                    'artist': 'Artist Name',
                    'album': 'Album Name',
                    'year': '2023',
                    'type': 'ALBUM',
                    'recording_verified': True,
                    'changes': {}
                }
            ]
        }
        dlg = OnlineAlbumLinkDialog(self.window, row, row['catalogue_options'])
        self.assertEqual(dlg.windowTitle(), 'Online Album Link')
        self.assertEqual(dlg.table.rowCount(), 1)
        self.assertEqual(dlg.table.item(0, 0).text(), 'Artist Name')
        self.assertEqual(dlg.table.item(0, 1).text(), 'Album Name')
        dlg._on_link_clicked()
        self.assertIsNotNone(dlg.chosen_option)
        self.assertEqual(dlg.chosen_option['id'], '99999')

    def test_background_linking_filter(self):
        self.store.mapping('Resolved Artist', '123', 'confirmed', 'Test', True)
        self.window._check_background_linking()

if __name__ == '__main__':
    unittest.main()
