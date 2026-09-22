"""Verify final counts, one-action cleanup and stable sorted-file filtering."""
import tempfile,time,json
from pathlib import Path
from PySide6.QtWidgets import QApplication
from PySide6.QtCore import Qt
from library_manager.ui import Window
from library_manager.core import Store,scan
from library_manager.library_workflows import inspect_snapshot
from test_maintenance import fixture
from mutagen.flac import FLAC

app=QApplication.instance() or QApplication([])
def settle(window):
    end=time.monotonic()+5
    while time.monotonic()<end:
        app.processEvents();time.sleep(.01)
        if not any(getattr(window,k,None) for k in ('worker','_preview_worker','_view_worker','_coverage_worker')):
            app.processEvents();return
    raise AssertionError('UI did not settle')
with tempfile.TemporaryDirectory() as directory:
    root=Path(directory)/'music';a=root/'A.flac';b=root/'Z.flac'
    for path in (a,b):fixture(path)
    audio=FLAC(a);audio['albumartist']=['Artist'];audio['date']=['2024/01/02'];audio.save()
    audio=FLAC(b);audio['albumartist']=['Local only'];audio['date']=['2024-01-02'];audio.save()
    store=Store(Path(directory)/'db');scan(store,root)
    store.mapping('Artist','1','confirmed','Test',True,extra_ids=['2','3'])
    favs=[{'id':'1','name':'Artist'},{'id':'2','name':'Artist'},{'id':'3','name':'Alias'},{'id':'4','name':'Absent'}]
    store.save_favourites('test',favs)
    window=Window(store,demo_mode=True);window.show();settle(window)
    window.cached_tidal_favourites=favs;window.render_favourites_page()
    assert window.card_favs_metric.text()=='2 favourites'
    assert window.card_favs_sub.text()=='1 in library · 1 missing locally'
    assert window.favs_filter_combo.itemText(0)=='All artists (3)'
    window.cached_tidal_favourites=[];window._update_card_favs_metric()
    assert window.card_favs_metric.text()=='0 favourites'
    window.cached_tidal_favourites=favs
    window.tools_plan=inspect_snapshot(root);window.tools_snapshots[str(root)]=window.tools_plan
    window.nav.setCurrentRow(5);window.tools_tabs.setCurrentIndex(1)
    window.tools_dates.setChecked(True);window.tools_discs.setChecked(True)
    assert not window.tools_dates.isChecked()
    window.tools_dates.setChecked(True);settle(window)
    window.tools_table.sortByColumn(0,Qt.SortOrder.DescendingOrder)
    window.tools_filter.setCurrentText('Unstandardised dates');window.filter_tools()
    visible=[window.tools_table.item(i,0).data(Qt.ItemDataRole.UserRole) for i in range(window.tools_table.rowCount()) if not window.tools_table.isRowHidden(i)]
    assert visible==[str(a.resolve())],visible
    before=window.activity_log.toPlainText()
    window.link_progress('Loading track details for selected release · Example')
    assert window.activity_log.toPlainText()==before
    window.link_progress('Searching for: Artist — Song')
    window.link_progress('Linked (1/2) · Artist — Song')
    window.link_progress('Request timed out · retrying')
    assert 'retrying' in window.activity_log.toPlainText()
    assert window.nav._item_tools.childCount()==6
    window.tools_tabs.setCurrentIndex(0);window.tools_filter.setCurrentText('All files');settle(window)
    for theme in ('Light','Dark'):
        window.appearance.setCurrentText(theme);settle(window)
        window.grab().save('/tmp/tibrary-release-'+theme.lower()+'.png')
    window.close();app.processEvents()
print('Passed: favourite identity counts, zero favourites, exclusive tools, sorted filtering, concise activity and both themes.')
