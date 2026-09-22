import sys,json,tempfile
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication,QTableWidget,QPushButton
from PySide6.QtCore import QItemSelectionModel,QEventLoop,QTimer
from library_manager.ui import Window,STYLE
from library_manager.core import Store,resolve
from test_batch_matching import artist,tracks
qt_errors=[]
sys.excepthook=lambda *args:qt_errors.append(args)
from ui_settings import isolate_settings
isolate_settings()
app=QApplication([]);app.setStyleSheet(STYLE)
with tempfile.TemporaryDirectory() as directory:
    store=Store(Path(directory)/'db.sqlite3')
    with store.connect() as db:
        for name in ('A','B','C'):
            for i,t in enumerate(tracks()): db.execute('INSERT INTO local_files VALUES(?,?,?,?,?,?,1)',(name+str(i),'root',0,0,json.dumps(dict(t,artist=name)),None))
    window=Window(store);window.show();calls=[]
    class Api:
        def search(self,name): return [dict(id=name,name=name)]
        def artist(self,ident,progress):calls.append(ident);return artist(ident,ident)
    window.api=lambda cancel,progress:Api()
    window.load_favourites=lambda cancel,progress:[]
    window.nav.setCurrentRow(1)
    for row in (0,1):window.artist_table.selectionModel().select(window.artist_table.model().index(row,0),QItemSelectionModel.SelectionFlag.Select|QItemSelectionModel.SelectionFlag.Rows)
    def finish():
        loop=QEventLoop();timer=QTimer();timer.setSingleShot(True);timer.timeout.connect(loop.quit);timer.start(3000)
        window.worker.finished.connect(loop.quit);loop.exec();app.processEvents();assert window.worker is None
    window.find_candidates();finish()
    assert calls==['A','B'],calls
    assert len(window.artist_table.selectionModel().selectedRows())==2
    assert len(store.rows('SELECT * FROM match_reviews'))==2
    window.match_all();finish();assert calls==['A','B','C']
    # Review a saved ambiguous result after the batch, then choose another identity.
    alternatives=resolve(tracks(),[artist('C-first','C'),artist('C-second','C')])[0]
    store.save_match_review('C','review',alternatives,'GB')
    store.mapping('C',None,'review','Two plausible candidates')
    window.refresh();window.artist_table.selectRow(2)
    def choose_second():
        dialog=app.activeModalWidget()
        dialog.findChild(QTableWidget).selectRow(1)
        dialog.accept()
    QTimer.singleShot(0,choose_second)
    window.review_candidates()
    mapping=store.rows("SELECT * FROM mappings WHERE artist='C'")[0]
    assert mapping['tidal_id']=='C-second' and mapping['manual']==1
    assert Store(store.path).rows("SELECT tidal_id FROM mappings WHERE artist='C'")[0]['tidal_id']=='C-second'
    assert window.nav.count()==7
    window.connection_dialog();app.processEvents();assert window.connection_page_widget.isVisible()
    window.settings_tabs.setCurrentIndex(0);app.processEvents()
    window.match_threshold.setValue(85)
    next(button for button in window.stack.currentWidget().findChildren(QPushButton) if button.text()=='Save matching settings').click()
    assert Store(store.path).match_preferences()['threshold']==85
    window.grab().save('/tmp/tidal-settings-page.png')
    window.nav.setCurrentRow(1);app.processEvents();window.grab().save('/tmp/tidal-batch-page.png')
    assert window.artist_table.columnCount()==6
    assert window.local_table.rowCount()==3
    assert window.local_table.item(0,1).text()=='1'
    assert window.artist_table.item(2,3).text()=='3'
    assert not window.activity_panel.isVisible()
    before=window.activity_log.toPlainText()
    window.log('Catalogue request · albums · attempt 1/4')
    assert window.activity_log.toPlainText()==before
    window.log('TIDAL returned HTTP 429 · retrying in 4 seconds')
    assert 'HTTP 429' in window.activity_log.toPlainText()
    assert not window.statusBar().currentMessage()
    assert not window.activity_panel.isVisible()
    window.details_button.click();app.processEvents()
    assert window.activity_panel.isVisible()
    window.details_button.click();app.processEvents()
    assert not window.activity_panel.isVisible()
    assert not qt_errors, qt_errors
    window.close()
print('Bulk UI passed: two selected artists, all-artist resume, selection preservation, connection and settings navigation.')
