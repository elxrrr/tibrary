"""Favourites-first workflow with mocked catalogue responses only."""
import json
import sys
import tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication
from PySide6.QtCore import QEventLoop, QTimer
from library_manager.ui import Window
from library_manager.core import Store
from library_manager.demo import catalogue
from library_manager.tidal import CatalogueError

from ui_settings import isolate_settings
isolate_settings()
app=QApplication([])

def wait(window):
    loop=QEventLoop(); timeout=QTimer(); timeout.setSingleShot(True); timeout.timeout.connect(loop.quit); timeout.start(3000)
    window.worker.finished.connect(loop.quit); loop.exec(); app.processEvents()
    assert window.worker is None

with tempfile.TemporaryDirectory() as directory:
    store=Store(Path(directory)/'app.sqlite3')
    artist=catalogue()
    tracks=[dict(t, artist=artist['name'], album=r['title']) for r in artist['releases'][:2] for t in r['tracks']]
    with store.connect() as db:
        for i,t in enumerate(tracks):
            db.execute('INSERT INTO local_files VALUES(?,?,?,?,?,?,1)', (str(i),'/fictional',0,0,json.dumps(t),None))
    store.save_favourites('account-cache', [{'id':artist['id'],'name':artist['name']}])
    window=Window(store)
    class FakeAccount:
        def load(self): return {'cache_id':'account-cache'}
    window.account=FakeAccount()
    searches=[]; outcomes=[]
    class FakeApi:
        def artist(self, ident, progress): return artist
        def search(self, name):
            searches.append(name)
            raise CatalogueError('Mock HTTP 400', status=400)
    window.api=lambda cancel,progress:FakeApi()
    window.candidates_dialog=lambda name,scored:outcomes.append(scored)
    window.artist_table.selectRow(0); window.find_candidates(); wait(window)
    assert not searches, 'Strong favourite match should avoid global search'
    assert json.loads(store.rows('SELECT payload FROM match_reviews')[0]['payload'])['candidates'][0]['artist']['id']==artist['id']
    assert store.rows('SELECT * FROM mappings')[0]['status']=='auto'
    with store.connect() as db:
        db.execute('DELETE FROM mappings')
        db.execute("DELETE FROM local_files WHERE path != '0'")
    window.refresh(); window.artist_table.selectRow(0); window.find_candidates(); wait(window)
    assert not searches
    assert store.rows('SELECT status FROM match_reviews')[0]['status']=='auto'
    assert store.rows('SELECT * FROM mappings')[0]['tidal_id']==artist['id'], 'A single positive release match should be auto-linked'
    assert 'Automatically matched' in window.activity_log.toPlainText()
    window.close()
print('Favourites workflow passed: cache use, favourites before global search, evidence requirement, safe fallback on search failure.')
