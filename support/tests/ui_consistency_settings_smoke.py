import faulthandler,tempfile,time,sys
from pathlib import Path
from unittest.mock import Mock,patch
from PySide6.QtWidgets import QApplication,QMessageBox,QGroupBox
from mutagen.flac import FLAC
from library_manager.core import Store,scan
from library_manager.ui import Window
from library_manager.library_workflows import inspect_snapshot
from library_manager.linking import saved_links
from ui_settings import isolate_settings
from test_maintenance import fixture

faulthandler.dump_traceback_later(30,exit=True)
isolate_settings();app=QApplication([]);errors=[]
def report(*args):errors.append(args);sys.__excepthook__(*args)
sys.excepthook=report

def settle(window):
    until=time.monotonic()+5;quiet=None
    while time.monotonic()<until:
        app.processEvents();time.sleep(.005)
        if not any((window.worker,window._preview_worker,window._view_worker,window._coverage_worker,window._link_worker)):
            if quiet is None:quiet=time.monotonic()
            if time.monotonic()-quiet>.04:return
        else:quiet=None
    raise AssertionError('Operation did not finish')

with tempfile.TemporaryDirectory() as directory:
    root=Path(directory).resolve()/'music';path=root/'Wrong/Album/song.flac';fixture(path)
    audio=FLAC(path);audio['isrc']=['GBTEST'];audio.save()
    store=Store(Path(directory)/'db');scan(store,root)
    track=dict(id='11',isrc='GBTEST',title='Incandescent',duration=1,track_number=1,disc_number=1)
    release_tracks=[track]+[dict(id=str(10+n),isrc=f'OTHER{n}',title=f'Other {n}',duration=1,track_number=n,disc_number=1) for n in range(2,7)]
    release=dict(id='1',artist='Wrong',title='Refraction (Remixes)',date='2024-05-01',track_count=6,tracks=release_tracks,tracks_loaded=True,tag_credits_checked=True,tag_checked_at=time.time(),metadata_schema=3,album_artists=['Wrong'],available=True)
    store.save_catalogue('1','GB',dict(id='1',releases=[release]));store.mapping('Wrong','1','confirmed','Test',True)
    api=Mock();api.track_tag_details.return_value=track
    window=Window(store);window.api=lambda *args:api;window.show()
    window.tools_plan=inspect_snapshot(root);window.invalidate_tools_plan();window.tools_tabs.setCurrentIndex(2);settle(window)
    with patch('library_manager.dj_metadata.DJMetadata.lookup',return_value=dict(bpm=125,key='G',key_scale='MAJOR')) as dj:
        window.fill_library_tags();settle(window)
        assert saved_links(store,'GB')[str(path)][1]['ids']['track_id']=='11'
        window.tools_tabs.setCurrentIndex(5);settle(window)
        assert window.tools_plan[0]['linked_ids']['track_id']=='11'
        window.tools_tabs.setCurrentIndex(2);settle(window);window.tools_select_changes()
        with patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):window.tools_apply()
        settle(window);api.reset_mock();dj.reset_mock()
        window.tools_tabs.setCurrentIndex(5);settle(window);window.start_linking();settle(window)
        assert window.tools_plan[0]['linked_ids']['track_id']=='11'
        assert api.mock_calls==[];dj.assert_not_called()
    assert not hasattr(window,'connection_overview')
    window.nav.setCurrentRow(6);window.appearance.setCurrentText('Dark');settle(window)
    assert 'border-right: 1px solid black' in app.styleSheet()
    assert window.download_quality.text()=='FLAC lossless · 16-bit / 44.1 kHz'
    assert len(window.findChildren(QGroupBox))>=9
    window.nav.setCurrentRow(1);settle(window);window.grab().save('/tmp/tidal-dark-table.png')
    window.nav.setCurrentRow(6);window.settings_tabs.setCurrentIndex(0);settle(window);window.grab().save('/tmp/tidal-settings-general.png')
    window.settings_tabs.setCurrentIndex(1);settle(window);window.grab().save('/tmp/tidal-settings-connections.png')
    assert not errors,errors
    window.close()
faulthandler.cancel_dump_traceback_later()
print('Passed: Add missing tags → central links → applied tags → no-request resume; native grouped Settings and dark headers.')
