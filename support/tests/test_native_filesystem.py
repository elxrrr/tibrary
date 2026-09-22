"""Native I/O contracts use disposable files only."""
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from library_manager.file_services import inventory, copy_flac_verified
from library_manager.core import Store, scan
from library_manager.maintenance import audio_digest, MaintenanceCancelled
from test_maintenance import fixture


class NativeFilesystemTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()

    def test_inventory_unicode_symlinks_timestamps_and_cancellation(self):
        music = self.root / 'music'; music.mkdir()
        track = music / 'Disc 01' / '01 - Odysée.FLAC'
        fixture(track)
        (music / 'cover.jpg').touch()
        (music / 'loop').symlink_to(music, target_is_directory=True)
        (music / 'alias.flac').symlink_to(track)
        rows, complete = inventory(music, {'.flac'})
        self.assertTrue(complete)
        st = track.stat()
        self.assertEqual(rows, [(str(track), st.st_size, st.st_mtime_ns)])
        self.assertEqual(inventory(music, {'.flac'}, lambda: True), ([], False))
        with self.assertRaises(OSError): inventory(self.root / 'absent', {'.flac'})

    def test_native_copy_digest_cancellation_and_no_overwrite(self):
        source = self.root / 'song.flac'; fixture(source)
        target = self.root / 'staged.flac'; target.touch()
        before = source.read_bytes()
        digest = copy_flac_verified(source, target, lambda: None)
        self.assertEqual(digest, audio_digest(source))
        self.assertEqual(target.read_bytes(), before)
        with self.assertRaises(ValueError): copy_flac_verified(source, target, lambda: None)
        target.unlink(); target.touch()
        calls = 0
        def cancel():
            nonlocal calls
            calls += 1
            if calls > 2: raise MaintenanceCancelled()
        with self.assertRaises(MaintenanceCancelled): copy_flac_verified(source, target, cancel)
        self.assertEqual(source.read_bytes(), before)
        target.unlink(); target.symlink_to(source)
        with self.assertRaises(ValueError): copy_flac_verified(source, target, lambda: None)
        self.assertEqual(source.read_bytes(), before)

    def test_scan_batches_and_retains_inventory_on_incomplete_walk(self):
        music = self.root / 'music'; music.mkdir()
        for i in range(130): (music / f'{i}.flac').touch()
        store = Store(self.root / 'db')
        reader = lambda p: {'artist': 'A', 'album': 'B', 'title': p.stem}
        with patch.object(store, 'connect', wraps=store.connect) as transactions:
            self.assertEqual(scan(store, music, reader)['read'], 130)
            self.assertLess(transactions.call_count, 12)
        self.assertEqual(scan(store, music, reader)['unchanged'], 130)
        (music / '0.flac').unlink()
        with patch('library_manager.file_services.inventory', side_effect=PermissionError('Unreadable folder')):
            self.assertEqual(scan(store, music, reader)['status'], 'offline or incomplete')
        self.assertEqual(len(store.tracks()), 130)
        self.assertEqual(scan(store, music, reader)['missing'], 1)
        self.assertEqual(len(store.tracks()), 129)
