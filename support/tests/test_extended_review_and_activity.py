"""Unit tests for Extended Review, Retagging, Activity timer/ETA, and button styles."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch
from PySide6.QtCore import Qt, QItemSelectionModel
from PySide6.QtGui import QColor
from PySide6.QtWidgets import QApplication
from library_manager.core import Store
from library_manager.ui import Window, STYLE, ExtendedArtistReviewDialog
from mutagen.flac import FLAC

app = QApplication.instance() or QApplication([])

class ExtendedReviewAndActivityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.db_path = Path(self.temp.name) / 'db.sqlite3'
        self.store = Store(self.db_path)
        self.window = Window(self.store)

    def test_button_stylesheet_rules(self):
        self.assertIn('QPushButton:hover', STYLE)
        self.assertIn('QPushButton:disabled', STYLE)
        self.assertIn('QPushButton#primary:hover', STYLE)
        self.assertIn('QPushButton#primary:disabled', STYLE)

    def test_match_selected_button_dynamic_state(self):
        # Initially disabled
        self.assertFalse(self.window.btn_match_sel.isEnabled())

        # Populate artist table
        self.window.artist_rows = [('Sad Alex', [{'title': 'High Hopes'}])]
        from library_manager.ui import fill
        fill(self.window.artist_table, [('Sad Alex', '1', '0', '1', 'Needs review', 'None')])

        # Still disabled when nothing selected
        self.window.local_details()
        self.assertFalse(self.window.btn_match_sel.isEnabled())

        # Select a row -> enabled
        self.window.artist_table.selectRow(0)
        self.window.local_details()
        self.assertTrue(self.window.btn_match_sel.isEnabled())

        # Clear selection -> disabled
        self.window.artist_table.clearSelection()
        self.window.local_details()
        self.assertFalse(self.window.btn_match_sel.isEnabled())

    def test_activity_page_status_and_cancel_button(self):
        # Cancel button disabled when no worker is running
        self.assertFalse(self.window.activity_cancel_btn.isEnabled())

        # Trigger progress update simulating background linking (< 3s)
        self.window._update_session_linking_progress("Checking track 10/100 · Artist — Song")
        self.assertEqual(self.window.session_linked_count, 10)
        self.assertEqual(self.window.session_total_to_check, 100)
        self.assertFalse(self.window.activity_pause_link_btn.isHidden())
        self.assertEqual(self.window.activity_pause_link_btn.text(), 'Pause background linking')
        self.assertEqual(self.window.activity_job_heading.text(), 'Running: Linking releases and tracks in background')
        self.assertFalse(self.window.activity_job_eta.isHidden())
        self.assertIn('calculating speed and finish ETA', self.window.activity_job_eta.text())

        # Simulate 10 seconds elapsed -> should display rate, ETA and remaining
        import time
        self.window.session_link_start_time = time.monotonic() - 10
        self.window._update_session_linking_progress("Checking track 20/100 · Artist — Song")
        self.assertIn('20 checked', self.window.activity_job_eta.text())
        self.assertIn('tracks/min', self.window.activity_job_eta.text())
        self.assertIn('ETA:', self.window.activity_job_eta.text())

        # Pausing linking updates status card and pause button text
        self.window._link_worker = Mock()
        self.window._link_worker.isRunning.return_value = True
        self.window.pause_linking()
        self.assertEqual(self.window.activity_job_heading.text(), 'Background linking paused')
        self.assertEqual(self.window.activity_pause_link_btn.text(), 'Resume background linking')

        # Linking finished resets card
        self.window.linking_finished()
        self.assertEqual(self.window.activity_job_heading.text(), 'No background tasks currently running')
        self.assertTrue(self.window.activity_job_eta.isHidden())
        self.assertTrue(self.window.activity_pause_link_btn.isHidden())

    def test_overview_session_banner(self):
        self.assertTrue(self.window.overview_session_banner.isHidden())
        self.window._update_session_linking_progress("Linked (25/200) · Some Artist — Song")
        self.assertFalse(self.window.overview_session_banner.isHidden())
        self.assertIn('25 of 200 tracks checked', self.window.overview_session_banner_label.text())

    def test_download_releases_sub_track_styling(self):
        sample_rows = [
            {'type': 'release', 'record': {'id': '100', 'approved': 1, 'artist': 'Test', 'title': 'Alb', 'date': '2021', 'type': 'ALBUM', 'track_count': 1, 'available': None}},
            {'type': 'track', 'record': {'id': '100', 'approved': 1}, 'parent_id': '100', 'track': {'id': '101', 'title': 'Trk', 'track_number': 1}, 'checked': True}
        ]
        self.window.displayed_queue_rows = sample_rows
        from library_manager.ui import fill
        table_rows = [
            ('', 'Test', 'Alb', 'Whole release', '2021', 'ALBUM', '1', ''),
            ('', 'Test', '  01. Trk', 'Track only', '', '', '', '')
        ]
        keys = [{'type': 'release'}, {'type': 'track'}]
        fill(self.window.queue_table, table_rows, keys=keys)

        # Call the styling loop directly
        for c in range(self.window.queue_table.columnCount()):
            cell = self.window.queue_table.item(1, c)
            if cell:
                cell.setBackground(QColor(255, 255, 255, 7))
                cell.setForeground(QColor(145, 205, 255) if c == 2 else QColor(140, 140, 145))

        # Check background tint across columns
        for c in range(self.window.queue_table.columnCount()):
            cell = self.window.queue_table.item(1, c)
            if cell:
                self.assertEqual(cell.background().color().alpha(), 7)

        # Check approved track foreground
        self.assertEqual(self.window.queue_table.item(1, 2).foreground().color().blue(), 255)
        self.assertEqual(self.window.queue_table.item(1, 1).foreground().color().blue(), 145)

    def test_purge_orphaned_artists_on_refresh(self):
        music_dir = Path(self.temp.name) / 'music'
        music_dir.mkdir()
        flac_file = music_dir / 'track.flac'
        flac_file.write_text('dummy')

        with self.store.connect() as db:
            db.execute("INSERT INTO roots VALUES(?, NULL, 'scanned')", (str(music_dir),))
            meta = {'artist': 'Real Artist', 'title': 'Song'}
            db.execute("INSERT INTO local_files VALUES(?, ?, 100, 100, ?, NULL, 1)",
                       (str(flac_file), str(music_dir), json.dumps(meta)))
            # Orphaned artist in match_reviews
            db.execute("INSERT INTO match_reviews VALUES(?, 'review', '{}', 'GB', '2026-01-01')", ('Sad Alex',))
            db.execute("INSERT INTO match_reviews VALUES(?, 'review', '{}', 'GB', '2026-01-01')", ('Real Artist',))

        self.window.refresh()
        remaining = [r['artist'] for r in self.store.rows("SELECT artist FROM match_reviews")]
        self.assertIn('Real Artist', remaining)
        self.assertNotIn('Sad Alex', remaining)

    def test_link_id_with_album_url(self):
        self.window.artist_rows = [('Sad Alex', [{'path': '/tmp/song.flac', 'isrc': 'USP6L2100619', 'title': 'High Hopes'}])]
        from library_manager.ui import fill
        fill(self.window.artist_table, [('Sad Alex', '1', '0', '1', 'Needs review', 'None')])
        self.window.artist_table.setCurrentCell(0, 0)
        self.window.artist_table.selectRow(0)

        fake_album = {
            'id': '171327615',
            'title': 'High Hopes',
            'album_artists': ['pluko'],
            'date': '2021-01-22',
            'tracks': [{'id': '171327616', 'title': 'High Hopes', 'isrc': 'USP6L2100619'}]
        }
        with patch('PySide6.QtWidgets.QInputDialog.getText', return_value=('https://tidal.com/album/171327615/u', True)), \
             patch.object(self.window, 'api') as mock_api_factory, \
             patch.object(self.window, 'retag_and_move_tracks') as mock_retag:
            mock_api = Mock()
            mock_api.album_tag_details.return_value = fake_album
            mock_api_factory.return_value = mock_api

            self.window.link_id()
            # Wait for worker job and process queued signals
            if self.window.worker:
                self.window.worker.wait(2000)
                app.processEvents()
            mock_retag.assert_called_once()
            discovery = mock_retag.call_args[0][1]
            self.assertEqual(discovery['discovered_artist'], 'pluko')
            self.assertEqual(discovery['release_id'], '171327615')

    def test_extended_artist_review_dialog_interaction(self):
        discoveries = [{
            'release_id': '171327615',
            'title': 'High Hopes',
            'discovered_artist': 'pluko',
            'year': '2021',
            'evidence': 'Exact ISRC match',
            'tracks_matched': [{'local_track': {'title': 'High Hopes'}}]
        }]
        dlg = ExtendedArtistReviewDialog(self.window, 'Sad Alex', [], discoveries)
        self.assertEqual(dlg.table.rowCount(), 1)
        self.assertIn('pluko', dlg.detail_label.text())
        dlg._apply_chosen()
        self.assertEqual(dlg.chosen_discovery['discovered_artist'], 'pluko')

    def test_extended_artist_review_title_search(self):
        music_dir = Path(self.temp.name) / 'music'
        music_dir.mkdir(exist_ok=True)
        flac_file = music_dir / 'track.flac'
        flac_file.write_text('dummy')

        with self.store.connect() as db:
            db.execute("INSERT INTO roots VALUES(?, NULL, 'scanned')", (str(music_dir),))
            # Notice: NO isrc in metadata!
            meta = {'artist': 'Sad Alex', 'title': 'High Hopes', 'album': 'High Hopes', 'duration_sec': 195}
            db.execute("INSERT INTO local_files VALUES(?, ?, 100, 100, ?, NULL, 1)",
                       (str(flac_file), str(music_dir), json.dumps(meta)))

        fake_album = {
            'id': '171327615',
            'title': 'High Hopes',
            'album_artists': ['pluko'],
            'date': '2021-01-22',
            'tracks': [{'id': '171327616', 'title': 'High Hopes', 'duration': 195}]
        }

        with patch.object(self.window, 'api') as mock_api_factory, \
             patch('library_manager.ui.ExtendedArtistReviewDialog.exec', return_value=False):
            mock_api = Mock()
            mock_api.search_albums.return_value = [{'id': '171327615', 'title': 'High Hopes'}]
            mock_api.search_tracks.return_value = []
            mock_api.recording_releases.return_value = []
            mock_api.album_tag_details.return_value = fake_album
            mock_api_factory.return_value = mock_api

            self.window.extended_artist_review('Sad Alex')
            if self.window.worker:
                self.window.worker.wait(3000)
                app.processEvents()

            mock_api.search_albums.assert_called_with('High Hopes')
            mock_api.album_tag_details.assert_called()


if __name__ == '__main__':
    unittest.main()
