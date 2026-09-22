"""Unit tests covering file menu controls, missing releases timeline filtering, and sibling linking."""
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, MagicMock
from PySide6.QtWidgets import QApplication
from library_manager.core import Store
from library_manager.ui import Window
from library_manager.view_data import filter_coverage
from library_manager.tag_review import check_album_tags

app = QApplication.instance() or QApplication([])


class FileMenuAndPolishTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.db_path = Path(self.temp.name) / 'db.sqlite3'
        self.store = Store(self.db_path)
        self.window = Window(self.store)

    def test_file_menu_actions_created(self):
        self.assertIsNotNone(self.window.menu_stop_job_act)
        self.assertIsNotNone(self.window.menu_pause_link_act)
        self.assertEqual(self.window.menu_stop_job_act.text(), "Stop current job")
        self.assertEqual(self.window.menu_pause_link_act.text(), "Pause background linking")

    def test_file_menu_actions_dynamic_state(self):
        # When idle
        self.window._update_file_menu_actions()
        self.assertFalse(self.window.menu_stop_job_act.isEnabled())
        self.assertFalse(self.window.menu_pause_link_act.isEnabled())

        # When background linking is running
        mock_link_worker = Mock()
        mock_link_worker.isRunning.return_value = True
        mock_link_worker.isInterruptionRequested.return_value = False
        self.window._link_worker = mock_link_worker
        self.window._link_enabled = True

        self.window._update_file_menu_actions()
        self.assertTrue(self.window.menu_stop_job_act.isEnabled())
        self.assertTrue(self.window.menu_pause_link_act.isEnabled())
        self.assertEqual(self.window.menu_pause_link_act.text(), "Pause background linking")

        # When background linking is paused
        mock_link_worker.isInterruptionRequested.return_value = True
        self.window._update_file_menu_actions()
        self.assertTrue(self.window.menu_pause_link_act.isEnabled())
        self.assertEqual(self.window.menu_pause_link_act.text(), "Resume background linking")

    def test_cancel_current_job(self):
        mock_worker = Mock()
        mock_worker.isRunning.return_value = True
        self.window.worker = mock_worker

        self.window.cancel_current_job()
        mock_worker.requestInterruption.assert_called_once()

    def test_timeline_available_count_filtering(self):
        # Test that filter_coverage filters available releases according to timeline
        # release 1: 2024 (newer than newest owned 2023) -> should be in available_count
        # release 2: 2020 (older than newest owned 2023) -> should NOT be in available_count for 'Newer than newest owned'
        data = {
            'compared': [
                {
                    'release': {'id': 'rel-newer', 'title': 'New Album', 'artist': 'Artist A', 'date': '2024-05-01', 'type': 'ALBUM', 'available': True},
                    'artist_ids': ['art-1'],
                    'local_artists': {'Artist A'},
                    'state': 'Missing release',
                    'quality_label': 'FLAC'
                },
                {
                    'release': {'id': 'rel-older', 'title': 'Old Album', 'artist': 'Artist A', 'date': '2020-01-01', 'type': 'ALBUM', 'available': True},
                    'artist_ids': ['art-1'],
                    'local_artists': {'Artist A'},
                    'state': 'Missing release',
                    'quality_label': 'FLAC'
                }
            ],
            'owned_dates': {'art-1': {'2023-01-01'}},
            'link_cache_days': 30
        }
        decisions = {}

        # 1. Timeline: Newer than newest owned
        filters_newer = ('Newer than newest owned', '', 'All statuses', 'All types', 'All copyrights')
        rows, visible, available_count = filter_coverage(data, decisions, filters_newer, demo=True)
        self.assertEqual(available_count, 1)
        self.assertEqual(len(visible), 1)
        self.assertEqual(visible[0]['release']['id'], 'rel-newer')

        # 2. Timeline: All missing releases
        filters_all = ('All missing releases', '', 'All statuses', 'All types', 'All copyrights')
        rows, visible, available_count = filter_coverage(data, decisions, filters_all, demo=True)
        self.assertEqual(available_count, 2)
        self.assertEqual(len(visible), 2)

    def test_sibling_batch_track_resolution(self):
        # Sibling tracks in the same directory: track 1 and track 2
        dir_path = Path(self.temp.name) / 'Artist' / 'Album'
        dir_path.mkdir(parents=True)
        track1_path = str(dir_path / '01 - Intro.flac')
        track2_path = str(dir_path / '02 - Outro.flac')

        # Mock API album response
        detailed_album = {
            'id': 12345,
            'title': 'Album',
            'album_artists': ['Artist'],
            'tracks': [
                {'id': 1001, 'title': 'Intro', 'track_number': 1, 'disc_number': 1, 'duration': 180, 'isrc': 'US1111111111'},
                {'id': 1002, 'title': 'Outro', 'track_number': 2, 'disc_number': 1, 'duration': 240, 'isrc': 'US2222222222'}
            ]
        }

        api = Mock()
        api.recording_releases.return_value = [{'id': 12345, 'title': 'Album', 'artist': 'Artist'}]
        api.album_tag_details.return_value = detailed_album

        plan = [
            {
                'path': track1_path,
                'tags': {
                    'title': ['Intro'],
                    'album': ['Album'],
                    'albumartist': ['Artist'],
                    'tracknumber': ['1'],
                    'discnumber': ['1'],
                    'isrc': ['US1111111111']
                },
                'duration': 180,
            },
            {
                'path': track2_path,
                'tags': {
                    'title': ['Outro'],
                    'album': ['Album'],
                    'albumartist': ['Artist'],
                    'tracknumber': ['2'],
                    'discnumber': ['1'],
                    'isrc': ['US2222222222']
                },
                'duration': 240,
            }
        ]

        results = []
        def on_result(row):
            results.append(row)

        reviewed = check_album_tags(plan, self.store, 'GB', api, on_result=on_result)

        # Both tracks should have been matched and linked
        self.assertEqual(len(reviewed), 2)
        self.assertEqual(reviewed[0]['catalogue_choice']['track_id'], '1001')
        self.assertEqual(reviewed[1]['catalogue_choice']['track_id'], '1002')
        # api.album_tag_details was only called once because track 2 was resolved as a sibling in-memory
        self.assertEqual(api.album_tag_details.call_count, 1)
        self.assertEqual(api.recording_releases.call_count, 1)


if __name__ == '__main__':
    unittest.main()
