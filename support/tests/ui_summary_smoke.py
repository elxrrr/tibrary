import sys
import json
import time
import tempfile
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication, QMessageBox
from PySide6.QtCore import QTimer, QEventLoop
from library_manager.ui import Window, STYLE
from library_manager.core import Store
from ui_settings import isolate_settings
isolate_settings()
app=QApplication([]);app.setStyleSheet(STYLE)
with tempfile.TemporaryDirectory() as directory:
    store=Store(Path(directory)/'app.sqlite3')
    local=dict(artist='Artist',album='Owned album',title='One',duration=180)
    with store.connect() as db: db.execute('INSERT INTO local_files VALUES(?,?,?,?,?,?,1)',('file','root',0,0,json.dumps(local),None))
    def release(ident,title,date): return dict(id=ident,artist='Artist',title=title,date=date,type='ALBUM',available=True,tracks=[],tracks_loaded=False,track_count=2,explicit=False,link_checked_at=time.time())
    artist=dict(id='10',name='Artist',releases=[release('1','Owned album','2020-01-01'),release('2','New album','2025-01-01')])
    store.save_catalogue('10','GB',artist);store.mapping('Artist','10','confirmed','User selected',True)
    # Two local artist spellings point to one catalogue: each release stays one row.
    with store.connect() as db:
        db.execute('INSERT INTO local_files VALUES(?,?,?,?,?,?,1)',('file2','root',0,0,json.dumps(dict(local,artist='Artist alias')),None))
    store.mapping('Artist alias','10','confirmed','User selected',True)
    window=Window(store);window.show();app.processEvents()
    assert window.timeline.currentText()=='Newer than newest owned'
    assert window.coverage_table.rowCount()==1
    window.timeline.setCurrentText('All dates')
    assert window.coverage_table.rowCount()==2
    assert window.coverage_table.item(0,4).text()=='2'
    window.coverage_table.selectRow(0);window.refresh_coverage()
    assert len(window.coverage_table.selectionModel().selectedRows())==1
    window.timeline.setCurrentText('Newer than newest owned')
    assert window.coverage_table.rowCount()==1
    window.coverage_table.selectRow(0)
    with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):window.queue_whole_release()
    assert window.queue_table.item(0,3).text()=='Whole release'
    assert json.loads(store.rows('SELECT payload FROM queue')[0]['payload'])['selected_tracks'] is None
    window.timeline.setCurrentText('All dates');window.coverage_table.selectRow(1)
    assert window.coverage_table.item(1,5).text()=='Present locally'
    class Api:
        def release_details(self,r): return dict(r,tracks_loaded=True,tracks=[dict(id='11',title='One',duration=180),dict(id='12',title='Two',duration=180)])
    window.api=lambda *args:Api()
    with patch.object(QMessageBox,'information'):
        window.check_release_tracks()
        loop=QEventLoop();timer=QTimer();timer.setSingleShot(True);timer.timeout.connect(loop.quit);timer.start(3000)
        window.worker.finished.connect(loop.quit);loop.exec();app.processEvents()
    assert window.worker is None
    assert window.coverage_table.item(1,5).text()=='Owned partial'
    window.timeline.setCurrentText('Incomplete albums')
    assert window.coverage_table.rowCount()==1
    assert window.coverage_table.item(0,5).text()=='Owned partial'
    from library_manager.ui import release_is_out
    assert not release_is_out(dict(date='2999-01-01'))
    assert not release_is_out(dict(date=''))
    assert release_is_out(dict(date='2020-01-01'))
    window.nav.setCurrentRow(3);app.processEvents();window.grab().save('/tmp/tidal-summary.png')
    window.close()
print('Summary UI passed: newer releases, whole-album queue, on-demand partial-track verification.')
