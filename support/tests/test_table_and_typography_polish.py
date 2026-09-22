"""Unit tests covering table resizing, sorting fix, typography escaping, and release type formatting."""
import unittest
from PySide6.QtCore import Qt
from PySide6.QtWidgets import QApplication, QWidget, QVBoxLayout
from library_manager.virtual_table import VirtualTable, RowsModel
from library_manager.ui import table, format_release_type, Window
from library_manager.core import Store
import tempfile
from pathlib import Path

app = QApplication.instance() or QApplication([])


class TableAndTypographyPolishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.db_path = Path(self.temp.name) / 'db.sqlite3'
        self.store = Store(self.db_path)
        self.window = Window(self.store)

    def test_format_release_type(self):
        self.assertEqual(format_release_type('SINGLE'), 'Single')
        self.assertEqual(format_release_type('ALBUM'), 'Album')
        self.assertEqual(format_release_type('EP'), 'EP')
        self.assertEqual(format_release_type('COMPILATION'), 'Compilation')
        self.assertEqual(format_release_type('TRACK'), 'Track')
        self.assertEqual(format_release_type('PODCAST'), 'Podcast')
        self.assertEqual(format_release_type(''), 'Unknown')
        self.assertEqual(format_release_type(None), 'Unknown')

    def test_table_stretching_and_row_height(self):
        t = table(['Col1', 'Col2', 'Col3'])
        self.assertTrue(t.horizontalHeader().stretchLastSection())
        self.assertEqual(t.verticalHeader().defaultSectionSize(), 30)

        # Virtual table also
        vt = table(['Col1', 'Col2'], virtual=True)
        self.assertTrue(vt.horizontalHeader().stretchLastSection())
        self.assertEqual(vt.verticalHeader().defaultSectionSize(), 30)

    def test_virtual_table_sort_flat_table_does_not_disappear(self):
        vt = VirtualTable(['File', 'Status', 'Match'])
        rows = [
            ('file_b.flac', 'Linked · Ready', 'Artist B'),
            ('file_a.flac', 'Needs choice', 'Artist A'),
            ('file_c.flac', '[Ignored] Unlinked', 'Artist C'),
        ]
        keys = ['/path/b', '/path/a', '/path/c']
        row_types = {2: 'ignored'}
        vt.model().replace(rows, keys, row_types=row_types)
        self.assertEqual(vt.rowCount(), 3)

        # Sort by File (col 0) ascending
        vt.model().sort(0, Qt.SortOrder.AscendingOrder)
        self.assertEqual(vt.rowCount(), 3)
        self.assertEqual(vt.model().rows[0][0], 'file_a.flac')
        self.assertEqual(vt.model().rows[1][0], 'file_b.flac')
        self.assertEqual(vt.model().rows[2][0], 'file_c.flac')
        self.assertEqual(vt.model().keys[2], '/path/c')
        self.assertEqual(vt.model().row_types.get(2), 'ignored')

        # Sort descending
        vt.model().sort(0, Qt.SortOrder.DescendingOrder)
        self.assertEqual(vt.rowCount(), 3)
        self.assertEqual(vt.model().rows[0][0], 'file_c.flac')
        self.assertEqual(vt.model().rows[1][0], 'file_b.flac')
        self.assertEqual(vt.model().rows[2][0], 'file_a.flac')

    def test_activity_table_columns_and_date_time_split(self):
        headers = [self.window.activity.horizontalHeaderItem(i).text() for i in range(self.window.activity.columnCount())]
        self.assertEqual(headers, ['Date', 'Time', 'Result', 'Summary'])

    def test_settings_details_ampersand_escaping(self):
        parent_widget = QWidget()
        layout = QVBoxLayout(parent_widget)
        details = Window.settings_details(layout, 'Template reference & tag variables')
        btn = layout.itemAt(0).widget()
        self.assertEqual(btn.text(), '▸ Template reference && tag variables')


if __name__ == '__main__':
    unittest.main()
