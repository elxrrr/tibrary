"""Test new controls and worker delivery using a fake OS store and fake TIDAL."""
import sys
import tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication, QLineEdit, QPushButton
from PySide6.QtCore import QEventLoop, QTimer
from library_manager.ui import Window, STYLE
from library_manager.credentials import Credentials
from library_manager.core import Store
from test_connection import MemoryKeychain

from ui_settings import isolate_settings
isolate_settings()
app = QApplication([]); app.setStyle('Fusion'); app.setStyleSheet(STYLE)

def wait_job(window):
    loop = QEventLoop(); deadline = QTimer(); deadline.setSingleShot(True)
    deadline.timeout.connect(loop.quit); deadline.start(3000)
    window.worker.finished.connect(loop.quit)
    loop.exec(); app.processEvents()
    assert window.worker is None, 'Background job did not finish'

with tempfile.TemporaryDirectory() as folder:
    credentials = Credentials(lambda: backend)
    backend = MemoryKeychain()
    window = Window(Store(Path(folder) / 'app.sqlite3'), credentials=credentials)
    window.show(); window.connection_dialog(); app.processEvents()
    dialog = window.connection_page_widget
    client = dialog.findChild(QLineEdit, 'client_id'); secret = dialog.findChild(QLineEdit, 'client_secret')
    assert secret.echoMode() == QLineEdit.EchoMode.Password
    client.setText('test-client'); secret.setText('test-secret')
    buttons = {b.text(): b for b in dialog.findChildren(QPushButton)}
    buttons['Save credentials'].click(); wait_job(window)
    assert credentials.get() == ('test-client', 'test-secret')
    assert not client.text() and not secret.text()
    replace=next(b for b in dialog.findChildren(QPushButton) if b.text()=='Replace saved credentials…')
    replace.click();app.processEvents()
    assert not client.isReadOnly() and not secret.isReadOnly()
    assert credentials.get()==('test-client','test-secret'),'Opening replacement must retain the saved pair'
    client.setText('replacement-client');secret.setText('replacement-secret');replace.click();wait_job(window)
    assert credentials.get()==('replacement-client','replacement-secret')
    class FakeApi:
        BASE = 'https://openapi.tidal.com/v2/'
        def authenticate(self): window.worker.message.emit('Authentication succeeded · mocked connection')
        def get(self,url): return {'data': []}
        def url(self,*args,**kwargs): return 'https://openapi.tidal.com/v2/searchResults'
    window.api = lambda cancel, progress: FakeApi()
    buttons['Test configured connection'].setEnabled(True)
    buttons['Test configured connection'].click(); wait_job(window)
    assert 'Catalogue search: connected' in window.activity_log.toPlainText()
    window.log('test-client test-secret replacement-client replacement-secret')
    assert 'test-secret' not in window.activity_log.toPlainText()
    assert 'test-client' not in window.activity_log.toPlainText()
    assert 'replacement-secret' not in window.activity_log.toPlainText()
    assert 'replacement-client' not in window.activity_log.toPlainText()
    dialog.grab().save('/tmp/tidal-connection.png')
    window.nav.setCurrentRow(0); window.details_button.click(); app.processEvents()
    assert window.activity_panel.isVisible()
    window.grab().save('/tmp/tidal-activity.png')
    window.close()
print('Connection UI passed: secure save, masked fields, mocked test, background progress, redacted activity.')
