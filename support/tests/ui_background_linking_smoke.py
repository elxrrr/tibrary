"""Local writes remain available during a slow mocked TIDAL request."""
import sys,tempfile,time,threading
from pathlib import Path
from unittest.mock import Mock,patch
from PySide6.QtWidgets import QApplication,QMessageBox
from mutagen.flac import FLAC
from library_manager.core import Store,scan
from library_manager.ui import Window
from library_manager.library_workflows import inspect_snapshot
from library_manager.linking import saved_links
from ui_settings import isolate_settings
from test_maintenance import fixture

import faulthandler
faulthandler.dump_traceback_later(30,exit=True)
isolate_settings();app=QApplication([]);errors=[]
def report(*args):
    errors.append(args);sys.__excepthook__(*args)
sys.excepthook=report
def pump(until,seconds=5):
    deadline=time.monotonic()+seconds
    settled=None
    while time.monotonic()<deadline:
        app.processEvents();time.sleep(.005)
        if until():
            if settled is None:settled=time.monotonic()
            if time.monotonic()-settled>.05:return
        else:settled=None
    raise AssertionError('Background work did not finish')

with tempfile.TemporaryDirectory() as folder:
    base=Path(folder).resolve();root=base/'music';path=root/'Wrong/Album/song.flac';fixture(path)
    audio=FLAC(path);audio.update(isrc=['GBTEST'],date=['2024/05/01'],discnumber=['01/02'],lyrics=['Remove'],bpm=['121'],key=['8A']);audio.save()
    store=Store(base/'db');scan(store,root)
    track=dict(id='11',isrc='GBTEST',title='Incandescent',duration=1,track_number=1,disc_number=1)
    release_tracks=[track]+[dict(id=str(10+n),isrc=f'OTHER{n}',title=f'Other {n}',duration=1,track_number=n,disc_number=1) for n in range(2,7)]
    release_tracks.append(dict(id='17',isrc='OTHER7',title='Other 7',duration=1,track_number=1,disc_number=2))
    release=dict(id='1',artist='Wrong',title='Refraction (Remixes)',date='2024-05-01',track_count=7,disc_count=2,tracks=release_tracks,tracks_loaded=True,
                 tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,album_artists=['Wrong'],available=True)
    store.save_catalogue('1','GB',dict(id='1',releases=[release]));store.mapping('Wrong','1','confirmed','Test',True)
    entered=threading.Event();unblock=threading.Event()
    def detail(_):entered.set();assert unblock.wait(15);return track
    api=Mock();api.track_tag_details.side_effect=detail;api.recording_releases.return_value=[]
    window=Window(store);window.api=lambda *args:api;window.show();window.nav.setCurrentRow(4)
    window.tools_plan=inspect_snapshot(root);window.invalidate_tools_plan();window.tools_tabs.setCurrentIndex(5)
    with patch('library_manager.dj_metadata.DJMetadata.lookup',return_value=dict(bpm=125,key='CSharp',key_scale='MINOR')):
        window.start_linking();pump(entered.is_set)
        assert window.tools_page_widget.isEnabled()
        window.tools_tabs.setCurrentIndex(1)
        for option in (window.tools_remove_lyrics, window.tools_dates, window.tools_discs):
            option.setChecked(True)
            pump(lambda: window._preview_worker is None)
            window.tools_select_changes()
            with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):window.tools_apply()
            pump(lambda:window.worker is None)
        assert window._link_worker is not None
        changed=FLAC(path);assert changed['date']==['2024-05-01'] and changed['discnumber']==['01']
        assert 'lyrics' not in changed and changed['bpm']==['121'] and changed.get('initialkey', changed.get('key'))==['8A']
        unblock.set()
        pump(lambda:window._link_worker is None and not window._link_restart)
        assert saved_links(store,'GB')[str(path)][1]['ids']['track_id']=='11'
        assert 'tidal_track_id' not in FLAC(path),'Background linking must not write file tags'
        window.tools_tabs.setCurrentIndex(2);app.processEvents()
        assert window.tools_plan[0]['linked_ids']['track_id']=='11'
        assert 'bpm' not in window.tools_plan[0]['changes']
        window.tools_tabs.setCurrentIndex(4);window.tools_organise_local.setChecked(True)
        assert '/Disc 01/01.01 - ' in window.tools_plan[0]['target']
        app.processEvents();window.grab().save('/tmp/tidal-local-cleanup.png')
        window.tools_tabs.setCurrentIndex(5);app.processEvents();window.grab().save('/tmp/tidal-background-links.png')
    assert not errors,errors
    window.close()
faulthandler.cancel_dump_traceback_later()
print('Passed: local cleanup during API wait, incremental relinking after edit, database-only matching, DJ tag preservation and disc-prefix preview.')
