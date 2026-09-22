"""Unit test for clean exit and drive/file operation safety."""
import tempfile
import unittest
from pathlib import Path
from unittest.mock import Mock, patch
from PySide6.QtWidgets import QApplication
from PySide6.QtGui import QCloseEvent
from library_manager.core import Store
from library_manager.ui import Window

app = QApplication.instance() or QApplication([])

class CleanExitSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'db')
        self.window = Window(self.store)

    def test_close_blocked_during_disk_operation(self):
        self.window.worker = Mock()
        self.window.worker.isRunning.return_value = True
        self.window._is_disk_operation = True

        event = QCloseEvent()
        with patch('PySide6.QtWidgets.QMessageBox.warning') as mock_warn:
            self.window.closeEvent(event)
            self.assertFalse(event.isAccepted())
            mock_warn.assert_called_once()

    def test_close_interrupts_non_disk_worker_without_blocking_ui(self):
        self.window._is_disk_operation = False
        mock_worker = Mock()
        mock_worker.isRunning.return_value = True
        self.window.worker = mock_worker

        event = QCloseEvent()
        with patch('library_manager.ui.QTimer.singleShot') as retry:
            self.window.closeEvent(event)
        self.assertFalse(event.isAccepted())
        mock_worker.requestInterruption.assert_called_once()
        mock_worker.wait.assert_not_called()
        retry.assert_called_once()

        mock_worker.isRunning.return_value = False
        finished = QCloseEvent()
        self.window.closeEvent(finished)
        self.assertTrue(finished.isAccepted())

    def test_close_shows_closing_overlay_when_workers_active(self):
        self.window._is_disk_operation = False
        mock_worker = Mock()
        mock_worker.isRunning.return_value = True
        self.window.worker = mock_worker

        event = QCloseEvent()
        self.window.closeEvent(event)
        self.assertFalse(self.window.closing_overlay.isHidden())

if __name__ == '__main__':
    unittest.main()
