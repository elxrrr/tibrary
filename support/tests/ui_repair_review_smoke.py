import tempfile,copy
from pathlib import Path
from PySide6.QtWidgets import QApplication
from ui_settings import isolate_settings
from library_manager.repair_review import AlbumRepairReview,review_groups
isolate_settings();app=QApplication([])
with tempfile.TemporaryDirectory() as folder:
    root=Path(folder)
    def row(number,options=True):
        return dict(root=str(root),path=str(root/'Wrong'/'Album'/f'{number}.flac'),blocked='',has_artwork=True,
            tags=dict(albumartist=['Wrong'],album=['Album'],title=[f'Track {number}'],tracknumber=[str(number)]),
            catalogue_note='Recording verified' if options else 'No verified correction',
            catalogue_options=[dict(id='1',artist='Right',album='Album',changes={'albumartist':['Right']},evidence='ISRC verified')] if options else [])
    rows=[row(1),row(2),row(3,False)];before=copy.deepcopy(rows)
    assert len(review_groups(rows))==2
    dialog=AlbumRepairReview(rows)
    dialog.use.click();assert dialog.position==1
    dialog.skip.click();assert len(dialog.plans)==2
    assert all(p['changes']['albumartist']==['Right'] and p['path']==p['target'] for p in dialog.plans)
    dialog.back.click();dialog.back.click();dialog.skip.click();dialog.skip.click()
    assert not dialog.plans and dialog.finish.isEnabled()
    assert rows==before,'Review must not modify source plans before acceptance'
    dialog.reject()
    dialog2 = AlbumRepairReview(rows)
    assert dialog2.save_early.isEnabled()
    dialog2.save_early.click()
    assert len(dialog2.plans) == 2
    assert dialog2.result() == AlbumRepairReview.DialogCode.Accepted
print('Guided review passed: album grouping, use/skip/back, preview and cancellation without edits.')
from PySide6.QtCore import QEventLoop,QTimer
from library_manager.ui import Window
from library_manager.core import Store,scan
from test_maintenance import fixture
from mutagen.flac import FLAC
from unittest.mock import Mock
with tempfile.TemporaryDirectory() as folder:
    root=Path(folder).resolve()/'music';path=root/'Wrong'/'Album'/'01.flac';fixture(path)
    audio=FLAC(path);audio['isrc']=['GBTEST'];audio.save();before=path.read_bytes()
    store=Store(Path(folder)/'db');scan(store,root)
    store.save_catalogue('1','GB',dict(releases=[dict(id='1',artist='Right',title='Refraction (Remixes)',date='2024-05-01',available=True,
        tracks_loaded=True,tag_credits_checked=True,metadata_schema=3,album_artists=['Right'],tracks=[dict(id='11',title='Incandescent',isrc='GBTEST')])]))
    window=Window(store);api=Mock();api.track_tag_details.return_value=dict(id='11',title='Incandescent',isrc='GBTEST',artists=['Canopy, Tom Finster']);window.api=lambda *args:api;window.show()
    assert not any(label.text()=='◈  COLLECTION' for label in window.findChildren(__import__('PySide6.QtWidgets',fromlist=['QLabel']).QLabel))
    window.guide_album_fixes(from_inbox=True)
    loop=QEventLoop();timer=QTimer();seen=[]
    def advance():
        dialog=app.activeModalWidget()
        if isinstance(dialog,AlbumRepairReview):
            seen.append(True);dialog.use.click();dialog.finish.click()
        if 'reviewed files selected' in window.tools_status.text():loop.quit()
    timer.timeout.connect(advance);timer.start(10);QTimer.singleShot(5000,loop.quit);loop.exec();timer.stop()
    assert seen and window.worker is None
    assert 'reviewed files selected' in window.tools_status.text(),window.tools_status.text()
    assert window.tools_selected()[0]['changes']['albumartist']==['Right']
    assert path.read_bytes()==before
    if window.worker: window.worker.wait(2000)
    if getattr(window, '_preview_worker', None): window._preview_worker.wait(2000)
    window.close(); app.processEvents()
print('Artist inbox → guided review → selected repair plan passed with mocked catalogue data.')
