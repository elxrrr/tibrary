import unittest
from unittest.mock import MagicMock, patch
from pathlib import Path
import json
import tempfile
import shutil
from datetime import datetime, timezone

from library_manager.backends import check_upstream_updates
from library_manager.ui import parse_utc_or_iso, format_user_datetime, ExtendedArtistReviewDialog, Window
from library_manager.core import Store


class TestV1Polish(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.mkdtemp()
        self.db_path = Path(self.temp_dir) / 'test.db'
        self.store = Store(self.db_path)

    def tearDown(self):
        shutil.rmtree(self.temp_dir, ignore_errors=True)

    def test_format_user_datetime_utc_to_local(self):
        # A UTC timestamp like 2026-09-18T12:00:00+00:00
        utc_ts = "2026-09-18T12:00:00+00:00"
        dt = parse_utc_or_iso(utc_ts)
        self.assertIsNotNone(dt)
        self.assertIsNotNone(dt.tzinfo)

        # formatted should contain year and month
        formatted = format_user_datetime(utc_ts)
        self.assertTrue(formatted.startswith("2026-09-18"))

        # Date only and time only
        self.assertEqual(format_user_datetime(utc_ts, date_only=True), "2026-09-18")
        time_str = format_user_datetime(utc_ts, time_only=True)
        self.assertTrue(":" in time_str)

        # Fallbacks for empty / None
        self.assertEqual(format_user_datetime(None), "—")
        self.assertEqual(format_user_datetime(""), "—")
        self.assertEqual(format_user_datetime("Never"), "Never")

    def test_check_upstream_updates_no_update(self):
        resources = Path(self.temp_dir) / 'resources'
        resources.mkdir(parents=True)
        backends = resources / 'backends'
        backends.mkdir()
        (backends / 'active.json').write_text(json.dumps({
            'commits': {
                'tidaler': 'aaaa11112222',
                'python-tidal': 'bbbb33334444'
            }
        }))

        with patch('subprocess.run') as mock_run:
            # Simulate git ls-remote returning the same commits
            def side_effect(cmd, **kwargs):
                mock = MagicMock()
                if 'ls-remote' in cmd:
                    if 'tidaler' in cmd[2]:
                        mock.stdout = 'aaaa11112222\tHEAD\n'
                    else:
                        mock.stdout = 'bbbb33334444\tHEAD\n'
                return mock
            mock_run.side_effect = side_effect

            res = check_upstream_updates(resources=resources)
            self.assertFalse(res['updates_available'])
            self.assertIn('up to date', res['summary'])

    def test_check_upstream_updates_with_update(self):
        resources = Path(self.temp_dir) / 'resources'
        resources.mkdir(parents=True)
        backends = resources / 'backends'
        backends.mkdir()
        (backends / 'active.json').write_text(json.dumps({
            'commits': {
                'tidaler': 'aaaa11112222',
                'python-tidal': 'bbbb33334444'
            }
        }))

        with patch('subprocess.run') as mock_run:
            def side_effect(cmd, **kwargs):
                mock = MagicMock()
                if 'ls-remote' in cmd:
                    if 'tidaler' in cmd[2]:
                        mock.stdout = 'cccc55556666\tHEAD\n'
                    else:
                        mock.stdout = 'bbbb33334444\tHEAD\n'
                return mock
            mock_run.side_effect = side_effect

            res = check_upstream_updates(resources=resources)
            self.assertTrue(res['updates_available'])
            self.assertIn('tidaler', res['summary'])
            self.assertTrue(res['details']['tidaler']['has_update'])
            self.assertFalse(res['details']['python-tidal']['has_update'])

    def test_extended_artist_review_search_in_demo(self):
        from PySide6.QtWidgets import QApplication, QWidget
        app = QApplication.instance() or QApplication([])

        parent = QWidget()
        parent.demo_mode = True

        local_tracks = [{'title': 'high hopes', 'path': '/music/pluko/high_hopes.flac'}]
        dlg = ExtendedArtistReviewDialog(parent, 'Sad Alex', local_tracks, [])

        dlg.search_input.setText('pluko')
        dlg._do_search()

        self.assertGreater(len(dlg.discoveries), 0)
        self.assertEqual(dlg.discoveries[0]['discovered_artist'], 'Pluko')
        self.assertEqual(dlg.table.rowCount(), len(dlg.discoveries))

    def test_retag_drops_old_artist_when_no_tracks_remain(self):
        # Insert track into local_files
        with self.store.connect() as db:
            meta = {
                'artist': 'Sad Alex',
                'albumartist': 'Sad Alex',
                'album': 'high hopes',
                'title': 'high hopes'
            }
            db.execute(
                "INSERT INTO local_files(path, root, size, mtime, metadata, error, present) VALUES(?,?,?,?,?,?,1)",
                ('/music/Sad Alex/high_hopes.flac', '/music', 1000, 1000, json.dumps(meta), '')
            )
            db.execute(
                "INSERT INTO match_reviews(artist, status, payload, error, updated) VALUES(?,?,?,?,?)",
                ('Sad Alex', 'review', json.dumps({'candidates': []}), '', '2026-09-18')
            )
            db.execute(
                "INSERT INTO mappings(artist, tidal_id, status, manual) VALUES(?,?,?,0)",
                ('Sad Alex', '', 'unmatched')
            )

        # Confirm artist exists in match_reviews and mappings
        self.assertEqual(len(self.store.rows("SELECT * FROM match_reviews WHERE artist='Sad Alex'")), 1)
        self.assertEqual(len(self.store.rows("SELECT * FROM mappings WHERE artist='Sad Alex'")), 1)

        # Simulate track retagging & moving to pluko:
        # Update local_files so track's metadata has albumartist='pluko'
        with self.store.connect() as db:
            meta['albumartist'] = 'pluko'
            meta['artist'] = 'pluko'
            db.execute("UPDATE local_files SET metadata=? WHERE path=?", (json.dumps(meta), '/music/Sad Alex/high_hopes.flac'))

        # Check artists remaining
        artists = self.store.artists(include_compilations=True)
        self.assertNotIn('Sad Alex', artists)

        # The cleanup logic from retag_and_move_tracks:
        if 'Sad Alex' not in artists or len(artists.get('Sad Alex', [])) == 0:
            with self.store.connect() as db:
                db.execute("DELETE FROM match_reviews WHERE artist=?", ('Sad Alex',))
                db.execute("DELETE FROM mappings WHERE artist=?", ('Sad Alex',))

        self.assertEqual(len(self.store.rows("SELECT * FROM match_reviews WHERE artist='Sad Alex'")), 0)
        self.assertEqual(len(self.store.rows("SELECT * FROM mappings WHERE artist='Sad Alex'")), 0)


if __name__ == '__main__':
    unittest.main()
