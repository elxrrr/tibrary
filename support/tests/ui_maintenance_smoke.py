import sys,tempfile
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication
from PySide6.QtCore import QEventLoop,QTimer,QSettings,Qt
from PySide6.QtGui import QStyleHints
from unittest.mock import patch
from library_manager.ui import Window,STYLE
from library_manager.core import Store,scan
from test_maintenance import fixture
errors=[];sys.excepthook=lambda *args:errors.append(args)
from ui_settings import isolate_settings
isolate_settings()
app=QApplication([]);app.setStyleSheet(STYLE)
with tempfile.TemporaryDirectory() as directory:
    QSettings.setDefaultFormat(QSettings.Format.IniFormat)
    QSettings.setPath(QSettings.Format.IniFormat,QSettings.Scope.UserScope,directory)
    root=Path(directory)/'music';fixture(root/'Artist'/'Album'/'song.flac')
    store=Store(Path(directory)/'db');scan(store,root)
    window=Window(store);window.show();window.nav.setCurrentRow(0);app.processEvents()
    assert len(window.metrics)==6
    assert window.metrics[0].text()=='1\nLocal tracks'
    assert window.metrics[1].text()=='1\nAlbum artists'
    assert window.metrics[2].text()=='1\nReleases'
    window.appearance.setCurrentText('Light');app.processEvents()
    assert window.settings.value('appearance') == 'Light'
    window.grab().save('/tmp/overview-light.png')
    window.appearance.setCurrentText('Dark');app.processEvents()
    assert 'background:' not in app.styleSheet()
    window.grab().save('/tmp/overview-dark.png')
    assert window.settings.value('appearance')=='Dark'
    window.appearance.setCurrentText('System')
    with patch.object(QStyleHints,'colorScheme',return_value=Qt.ColorScheme.Dark):
        app.styleHints().colorSchemeChanged.emit(Qt.ColorScheme.Dark)
        assert 'background:' not in app.styleSheet()
    window.nav.setCurrentRow(5);window.tools_tabs.setCurrentIndex(1);window.tools_dates.setChecked(True);window.inspect_library()
    loop=QEventLoop();timer=QTimer();timer.setSingleShot(True);timer.timeout.connect(loop.quit);timer.start(3000)
    window.worker.finished.connect(loop.quit);loop.exec();app.processEvents()
    assert window.worker is None
    assert window.tools_table.rowCount()==1
    assert '2024-05-01' in window.tools_table.item(0,2).text()
    window.tools_select_changes();assert len(window.tools_selected())==1
    window.grab().save('/tmp/tools-health.png')
    assert not errors,errors
    window.close()
print('Overview counters and Library Tools inspection preview passed.')
