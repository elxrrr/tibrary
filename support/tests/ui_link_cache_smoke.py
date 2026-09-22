"""Native navigation/cache smoke using temporary files and mocked TIDAL only."""
import json
import sys
import tempfile
import time
from pathlib import Path
from unittest.mock import Mock, patch
from PySide6.QtWidgets import QApplication
from library_manager.core import Store, scan
from library_manager.ui import Window
from library_manager.library_workflows import inspect_snapshot
from ui_settings import isolate_settings
from test_maintenance import fixture

isolate_settings()
app=QApplication([])
errors=[]
sys.excepthook=lambda *args:(errors.append(args),sys.__excepthook__(*args))

def finish(window):
    deadline=time.monotonic()+5
    while any((window.worker,window._preview_worker,window._view_worker,window._coverage_worker)) and time.monotonic()<deadline:
        app.processEvents();time.sleep(.005)
    app.processEvents()
    assert not any((window.worker,window._preview_worker,window._view_worker,window._coverage_worker))

with tempfile.TemporaryDirectory() as directory:
    base=Path(directory).resolve();root=base/'music';fixture(root/'Wrong/Album/song.flac')
    store=Store(base/'db');scan(store,root)
    artist=next(iter(store.artists()))
    releases=[dict(id=str(n),artist=artist,title=f'New release {n}',date='2025-01-01',type='ALBUM',
                   track_count=2,explicit=False,tracks=[],tracks_loaded=False,available=True,
                   link_checked_at=time.time()-age*86400 if age else 0) for n,age in [(1,40),(2,10),(3,0)]]
    store.save_catalogue('a','GB',dict(id='a',name=artist,releases=releases))
    store.mapping(artist,'a','confirmed','Test',True)
    window=Window(store);window.show();window.timeline.setCurrentText('All dates');finish(window)
    window.api=Mock(side_effect=AssertionError('Navigation must stay offline'))
    for page in (0,2,4,5,2):window.nav.setCurrentRow(page);app.processEvents()
    assert window.worker is None and window.api.call_count==0
    assert {i['release']['id'] for i in window.coverage_rows}=={'1','2','3'}  # Unchecked candidates stay inspectable.
    window.link_cache_age.setCurrentIndex(window.link_cache_age.findData(0));finish(window)
    assert {i['release']['id'] for i in window.coverage_rows}=={'1','2','3'}
    assert Store(base/'db').preferences('release_links')['max_age_days']==0
    assert window.api.call_count==0
    calls=[]
    class Api:
        def release_availability(self,release,force=False):
            calls.append(release['id']);return dict(release,link_checked_at=time.time())
    window.api=lambda *args:Api()
    window.check_release_links();finish(window)
    assert calls==['3']
    window.check_release_links();assert calls==['3'] and window.worker is None
    index=next(i for i,row in enumerate(window.coverage_rows) if row['release']['id']=='1')
    window.coverage_table.selectRow(index)
    window.check_release_links(force_selected=True);finish(window)
    assert calls==['3','1']
    before=store.cache('a','GB')['releases'][0]['link_checked_at']
    class FailingApi:
        def release_availability(self,*args,**kwargs):raise RuntimeError('Mock timeout')
    window.api=lambda *args:FailingApi()
    index=next(i for i,row in enumerate(window.coverage_rows) if row['release']['id']=='1')
    window.coverage_table.selectRow(index)
    with patch('library_manager.diagnostics.record_failure'):
        window.check_release_links(force_selected=True);finish(window)
    assert store.cache('a','GB')['releases'][0]['link_checked_at']==before
    window.tools_plan=inspect_snapshot(root)
    window.tools_tabs.setCurrentIndex(1);finish(window)
    assert 'tags' in window._tool_counts
    with patch('library_manager.library_workflows.workflow_plan',side_effect=AssertionError('Summary rebuilt previews')),patch('library_manager.freshness.prepare_library',side_effect=AssertionError('Summary scanned files')),patch('library_manager.ui.fill',side_effect=AssertionError('Summary filled hidden table')):
        window.tools_tabs.setCurrentIndex(0);finish(window)
    assert 'file' in window.tools_summary_buttons['tags'].text()
    assert not errors,errors
    window.close()
print('Passed: offline navigation/Summary, persistent expiry preference, cached batches, selected recheck, timeout preserves cache.')
