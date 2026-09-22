"""A synthetic large preview: no library scan, credentials, network or music writes."""
import copy,tempfile,time,sys
from pathlib import Path
from unittest.mock import patch,Mock
from PySide6.QtCore import QTimer,QEventLoop,QThread
from PySide6.QtWidgets import QApplication
from library_manager.ui import Window
from library_manager.core import Store
from library_manager.maintenance import inspect_file
from ui_settings import isolate_settings
from test_maintenance import fixture
isolate_settings();app=QApplication([]);errors=[]
def on_error(*args):
    errors.append(args);sys.__excepthook__(*args)
sys.excepthook=on_error
with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as folder:
    base=Path(folder).resolve();root=base/'music';path=root/'Artist/Album/1.flac';fixture(path)
    store=Store(base/'db');window=Window(store);window.show()
    template=inspect_file(path,root);rows=[]
    for i in range(10000):
        row=copy.deepcopy(template);row['path']=str(root/f'Artist/Album/{i}.flac');rows.append(row)
    window.tools_root.addItem(str(root),str(root));window.tools_tabs.setCurrentIndex(2)
    window._prepared_mode=(str(root),'metadata');window.tools_plan=rows;window.tools_snapshots[str(root)]=rows
    beats=[];timer=QTimer();timer.setInterval(20);timer.timeout.connect(lambda:beats.append(time.monotonic()));timer.start()
    loop=QEventLoop();poll=QTimer();poll.setInterval(20)
    poll.timeout.connect(lambda:loop.quit() if window._preview_worker is None and window._preview_pending is None else None)
    poll.start();start=time.monotonic();window.invalidate_tools_plan();assert time.monotonic()-start<.2,'Preparing a large preview blocked the caller';QTimer.singleShot(10000,loop.quit);loop.exec()
    assert window._preview_worker is None,'Preview did not finish'
    assert window.tools_table.rowCount()==10000
    assert beats and time.monotonic()-start<3,'Large preview stalled the event loop'
    preview_duration=time.monotonic()-start
    window.tools_select_changes()
    assert len(window.tools_selected())==0
    # Job callbacks must run on the application's GUI thread, not the worker.
    seen=[];done=QEventLoop()
    window.job(lambda cancel,progress:(time.sleep(.4) or 'Finished test'),lambda result:seen.append(QThread.currentThread()==app.thread()),label='Mock operation')
    window.worker.finished.connect(done.quit);QTimer.singleShot(5000,done.quit);done.exec();app.processEvents()
    assert seen==[True]
    assert len(beats)>5,'UI timer stalled during the simulated slow job'
    assert not errors,errors
    duration=time.monotonic()-start
    gaps=[b-a for a,b in zip(beats,beats[1:])]
    print(f'10,000-row preview and GUI-thread callbacks passed; {duration:.2f}s elapsed, {len(beats)} UI heartbeats, largest gap {max(gaps,default=0):.2f}s.')
    if window.worker: window.worker.wait(2000)
    if getattr(window, '_preview_worker', None): window._preview_worker.wait(2000)
    timer.stop();poll.stop();window.close()
    app.processEvents()
