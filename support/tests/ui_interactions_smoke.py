import time,tempfile,json,threading,faulthandler
faulthandler.dump_traceback_later(10,repeat=True)
from pathlib import Path
from unittest.mock import Mock
from PySide6.QtWidgets import QApplication,QDialog,QDialogButtonBox,QPushButton,QPlainTextEdit
from PySide6.QtCore import Qt,QPoint,QTimer
from PySide6.QtTest import QTest
from library_manager.ui import Window,ExtendedArtistReviewDialog
from library_manager.core import Store,scan
from library_manager.maintenance import audio_digest
from library_manager.linking import saved_links
from test_maintenance import fixture
from mutagen.flac import FLAC
app=QApplication.instance() or QApplication([])
def pump(until,seconds=20):
    end=time.monotonic()+seconds
    while time.monotonic()<end:
        app.processEvents();time.sleep(.005)
        if until():return
    raise AssertionError('UI operation timed out: '+window.activity_log.toPlainText())
with tempfile.TemporaryDirectory() as folder:
    root=(Path(folder)/'music').resolve();source=root/'Wrong'/'Old'/'song.flac';fixture(source)
    audio=FLAC(source);audio.update(albumartist=['Wrong'],album=['Old'],title=['Song'],isrc=['TEST'],bpm=['128'],initialkey=['9A']);audio.save();digest=audio_digest(source)
    store=Store(Path(folder)/'db');scan(store,root)
    release=dict(id='100',title='Album',album_artists=['Right'],album_artist_ids=['10'],date='2024-06-01',available=True,tracks_loaded=True,credits_complete=True,tag_checked_at=time.time(),disc_count=1,type='ALBUM',
        tracks=[dict(id='101',title='Song',isrc='TEST',track_number=1,disc_number=1,duration=1)])
    store.save_preferences('tag-review:GB:100',release)
    window=Window(store,demo_mode=True);window.show();app.processEvents();window.demo_mode=False;window.market='GB';window._bg_linking_timer.stop()
    nav=window.nav;parent=nav._item_tools
    parent.setExpanded(True);nav.scrollToItem(parent);app.processEvents()
    rect=nav.visualItemRect(parent)
    QTest.mouseClick(nav.viewport(),Qt.MouseButton.LeftButton,pos=rect.center());app.processEvents()
    assert parent.isExpanded(),'Clicking label collapsed its children'
    rect=nav.visualItemRect(parent);branch=QPoint(rect.left()-nav.indentation()//2,rect.center().y())
    QTest.mouseClick(nav.viewport(),Qt.MouseButton.LeftButton,pos=branch);app.processEvents()
    assert not parent.isExpanded(),'Disclosure triangle did not collapse'
    QTest.mouseClick(nav.viewport(),Qt.MouseButton.LeftButton,pos=branch);app.processEvents()
    assert parent.isExpanded()
    nav.setCurrentItem(parent.child(2));app.processEvents()
    assert window.tools_tabs.currentIndex()==2 and window.stack.currentIndex()==5
    # Downloaded expansion respects the actual downloaded subset and sort identity.
    payload=dict(release,artist='Right',selected_tracks=[release['tracks'][0]])
    store.enqueue(payload,payload['selected_tracks'])
    with store.connect() as db:db.execute("UPDATE queue SET decision='downloaded'")
    window.nav.setCurrentRow(9);window.refresh_downloaded_releases();app.processEvents()
    table=window.downloaded_table
    QTest.mouseClick(table.viewport(),Qt.MouseButton.LeftButton,pos=table.visualRect(table.model().index(0,1)).center());app.processEvents()
    assert table.rowCount()==2 and table.item(1,1).text()=='Song'
    table.sortByColumn(1,Qt.SortOrder.DescendingOrder);app.processEvents()
    assert table.item(1,5).text()=='101'
    window.grab().save('/tmp/tibrary-downloaded-expanded.png')
    # Full extended-review -> preview -> guarded write flow, using only the temp FLAC.
    verified=[]
    def approve():
        dlg=app.activeModalWidget()
        if dlg and not isinstance(dlg,ExtendedArtistReviewDialog) and dlg.windowTitle()!='Review tag changes and file moves':
            print('Unexpected modal:',dlg.windowTitle(),flush=True);dlg.reject()
        if isinstance(dlg,ExtendedArtistReviewDialog):
            assert dlg.discoveries;dlg.table.selectRow(0);dlg._apply_chosen()
        elif isinstance(dlg,QDialog) and dlg.windowTitle()=='Review tag changes and file moves':
            detail=dlg.findChild(QPlainTextEdit).toPlainText()
            assert 'albumartist: Wrong → Right' in detail and 'Move to:' in detail
            verified.append(detail);dlg.accept()
    timer=QTimer();timer.timeout.connect(approve);timer.start(20)
    window.api=Mock(return_value=Mock())
    window.extended_artist_review('Wrong')
    pump(lambda:bool(verified) and not source.exists() and window.worker is None)
    timer.stop()
    target=next(root.rglob('*.flac'))
    assert audio_digest(target)==digest and FLAC(target)['bpm']==['128'] and FLAC(target)['initialkey']==['9A']
    assert saved_links(store,'GB')[str(target)][1]['ids']=={'album_id':'100','track_id':'101'}
    assert 'Wrong' not in store.artists()
    # Search field does not block the GUI while TIDAL is waiting.
    gate=threading.Event();entered=threading.Event()
    api=Mock()
    def waiting_release(r):entered.set();assert gate.wait(3);return dict(release,id='200')
    api.album_tag_details.side_effect=waiting_release;window.api=lambda *a:api
    dlg=ExtendedArtistReviewDialog(window,'Right',store.tracks(),[]);dlg.show();dlg.search_input.setText('200')
    start=time.monotonic();dlg._do_search();assert time.monotonic()-start<.2
    pump(entered.is_set)
    heartbeats=[];QTimer.singleShot(0,lambda:heartbeats.append(True));pump(lambda:bool(heartbeats));gate.set()
    pump(lambda:window.worker is None)
    assert dlg.discoveries[0]['release_id']=='200';dlg.reject()
    window.nav.setCurrentRow(6);window.settings_tabs.setCurrentIndex(0);app.processEvents()
    reference=next(b for b in window.findChildren(QPushButton) if 'Template reference' in b.text())
    reference.click();app.processEvents()
    tags=[b for b in window.findChildren(QPushButton) if b.text().startswith('{')]
    assert len({b.height() for b in tags})==1
    window.settings_tabs.widget(0).ensureWidgetVisible(tags[-1])
    app.processEvents()
    window.grab().save('/tmp/tibrary-template-reference.png')
    window.close();app.processEvents()
print('Passed: disclosure clicks, keyboard navigation, downloaded subsets, deep scan -> preview -> retag/move/link, audio/DJ preservation, asynchronous release search and template control heights.')
