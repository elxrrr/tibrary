import tempfile,sys
from pathlib import Path
from unittest.mock import patch
from PySide6.QtCore import QEventLoop,QTimer
from PySide6.QtWidgets import QApplication,QInputDialog,QMessageBox
from mutagen.flac import FLAC
from library_manager.ui import Window
from library_manager.core import Store,scan
from library_manager.library_workflows import inspect_snapshot
from ui_settings import isolate_settings
from test_maintenance import fixture
isolate_settings();app=QApplication([])
errors=[];sys.excepthook=lambda *args:errors.append(args)
def finish(window):
    loop=QEventLoop();window.worker.finished.connect(loop.quit);QTimer.singleShot(5000,loop.quit);loop.exec();app.processEvents()
    assert window.worker is None
with tempfile.TemporaryDirectory() as folder:
    base=Path(folder).resolve();root=base/'music';path=root/'Wrong/Album/song.flac';fixture(path)
    audio=FLAC(path);audio['bpm']=['121.000000'];audio['key']=['8A'];audio['lyrics']=['Existing lyrics'];audio.save()
    store=Store(base/'db');scan(store,root);window=Window(store);window.show();window.nav.setCurrentRow(5)
    window.tools_plan=inspect_snapshot(root);window.invalidate_tools_plan()
    assert not window.settings_tabs.documentMode() and not window.tools_tabs.documentMode()
    for index in range(5):
        window.tools_tabs.setCurrentIndex(index);app.processEvents()
        assert window.tools_workspace.isVisible()
        window.grab().save(f'/tmp/tools-workflow-{app.platformName()}-{index}.png')
    window.tools_tabs.setCurrentIndex(1)
    assert not window.tools_remove_lyrics.isChecked()
    assert not hasattr(window,'tools_lyrics') and not hasattr(window,'download_lyrics')
    window.tools_remove_lyrics.setChecked(True)
    assert window.tools_plan[0]['changes']['lyrics']==[]
    with patch.object(QInputDialog,'getText',return_value=('Mo Falk',True)):window.tools_set_artist()
    assert window.tools_plan[0]['target']==str(path)
    window.tools_tabs.setCurrentIndex(4)
    assert window.tools_plan[0]['changes']=={} and '/Wrong/' in window.tools_plan[0]['target']
    window.tools_tabs.setCurrentIndex(1);window.tools_select_changes();app.processEvents()
    assert 'Album artist (library grouping): Wrong' in window.tools_detail.toPlainText()
    assert 'Mo Falk' in window.tools_detail.toPlainText()
    window.grab().save(f'/tmp/tools-workflow-{app.platformName()}-preview.png')
    window.credentials.session=('mock-client','mock-secret')
    with patch('library_manager.ui.BatchMatcher') as matcher:
        matcher.return_value.run.return_value='Artist relinking checked'
        with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):window.tools_apply()
        finish(window)
        matcher.assert_not_called()  # Local cleanup must never contact TIDAL.
    assert path.exists() and FLAC(path)['albumartist']==['Mo Falk']
    assert 'lyrics' not in FLAC(path)
    assert set(store.artists())=={'Mo Falk'}
    window.tools_tabs.setCurrentIndex(4);window.tools_org_layout.setChecked(True);window.tools_select_changes()
    target=Path(window.tools_plan[0]['target']);assert '/Mo Falk/' in str(target)
    before=path.read_bytes()
    with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):window.tools_apply()
    finish(window);assert not path.exists() and target.read_bytes()==before
    assert FLAC(target)['bpm']==['121.000000'] and FLAC(target).get('initialkey', FLAC(target).get('key'))==['8A']
    # An external retag and subsequent rescan reconcile both the inbox and inspection.
    audio=FLAC(target);audio['albumartist']=['External correction'];audio.save()
    window.nav.setCurrentRow(0);window.rescan();finish(window)
    assert set(store.artists())=={'External correction'}
    assert window.tools_plan[0]['tags']['albumartist']==['External correction']
    target.unlink();window.rescan();finish(window)
    assert not window.tools_plan and not store.artists()
    window.nav.setCurrentRow(6);window.settings_tabs.setCurrentIndex(0);app.processEvents();window.grab().save(f'/tmp/settings-selector-{app.platformName()}.png')
    assert not errors,errors
    window.close()
print('Independent tag/move workflows, explicit scope, current previews, artist index refresh and native selectors passed.')
