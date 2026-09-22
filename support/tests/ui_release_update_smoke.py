import tempfile,time,json
from pathlib import Path
from unittest.mock import Mock,patch
from PySide6.QtWidgets import QApplication,QMessageBox
from PySide6.QtCore import QTimer
from mutagen.flac import FLAC
from library_manager.ui import Window
from library_manager.core import Store,scan
from test_release_update import audio_file,remote
app=QApplication.instance() or QApplication([])
def wait_for(predicate):
    end=time.monotonic()+20
    while time.monotonic()<end:
        app.processEvents();time.sleep(.005)
        if predicate():return
    raise AssertionError('UI timed out: '+window.activity_log.toPlainText())
with tempfile.TemporaryDirectory() as tmp:
    root=Path(tmp).resolve()/'music';path=audio_file(root/'Artist/Single/01 - Song 1.flac',1,1,'Song 1','TEST1')
    tags=FLAC(path);tags.update(mqaencoder=['MQA'],tidal_album_id=['100'],tidal_track_id=['11']);tags.save();original=path.read_bytes()
    store=Store(Path(tmp)/'db');scan(store,root);store.mapping('Artist','a','confirmed','fixture',True)
    release=remote()
    for track in release['tracks']:track['artists']=['Artist','Guest']
    store.save_catalogue('a','GB',dict(id='a',releases=[release]))
    window=Window(store,demo_mode=True);window.resize(1340,900);window.show();app.processEvents()
    window.demo_mode=False;window.market='GB';window._bg_linking_timer.stop()
    api=Mock();api.album_tag_details.return_value=release;window.api=lambda *args:api
    window.nav.setCurrentRow(10);app.processEvents();window.mqa_page.inspect()
    wait_for(lambda:window.worker is None and bool(window.mqa_page.rows))
    assert window.mqa_page.rows[0]['status']=='MQA tags'
    window.mqa_page.table.selectRow(0);window.mqa_page.act()
    wait_for(lambda:window.worker is None and bool(store.rows('SELECT * FROM queue')))
    queued=store.rows('SELECT * FROM queue')[0];assert not queued['approved'];assert len(json.loads(queued['payload'])['selected_tracks'])==1
    window.grab().save('/tmp/tibrary-mqa-audit.png')
    window.nav.setCurrentRow(12);app.processEvents();window.online_optimizations_page.inspect()
    wait_for(lambda:window.worker is None and bool(window.online_optimizations_page.rows))
    assert len(window.online_optimizations_page.rows)==1
    window.online_optimizations_page.table.selectRow(0)
    with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):window.online_optimizations_page.act()
    queued=json.loads(store.rows('SELECT payload FROM queue')[0]['payload']);assert queued['selected_tracks'] is None
    assert queued['preserve_local_dj']['11']['initialkey']==['8A']
    window.grab().save('/tmp/tibrary-optimizations.png')
    window.nav.setCurrentRow(5);window.tools_tabs.setCurrentIndex(0);app.processEvents()
    window.grab().save('/tmp/tibrary-library-health.png')
    window.nav.setCurrentRow(6);window.settings_tabs.setCurrentIndex(2);app.processEvents()
    window.appearance.setCurrentText('Dark');app.processEvents();window.grab().save('/tmp/tibrary-download-settings-dark.png')
    window.appearance.setCurrentText('Light');app.processEvents();window.grab().save('/tmp/tibrary-download-settings.png')
    assert path.read_bytes()==original
    window.close();app.processEvents()
print('Passed native MQA inspection, replacement queue, cached optimizer discovery, complete replacement queue, settings and source preservation.')
