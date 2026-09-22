import tempfile
from pathlib import Path
from PySide6.QtWidgets import QApplication
from PySide6.QtGui import QPalette
from PySide6.QtCore import QEventLoop,QTimer
from library_manager.ui import Window
from library_manager.core import Store,scan
from library_manager.maintenance import plan_library
from ui_settings import isolate_settings
from test_maintenance import fixture
from mutagen.flac import FLAC
isolate_settings();app=QApplication([])
with tempfile.TemporaryDirectory() as folder:
    base=Path(folder).resolve();root=base/'music'
    for name in ['one','two']:fixture(root/'Artist/Album'/f'{name}.flac')
    audio=FLAC(root/'Artist/Album/one.flac');audio['key']=['8A'];audio['bpm']=['125'];audio.save()
    store=Store(base/'db');scan(store,root);window=Window(store);window.show()
    window.nav.setCurrentRow(6);app.processEvents()
    general=window.settings_tabs.widget(0);connections=window.settings_tabs.widget(1)
    assert general.viewport().backgroundRole()==connections.viewport().backgroundRole()==QPalette.ColorRole.Window
    window.grab().save('/tmp/release-settings.png')
    window.settings_tabs.setCurrentIndex(1);app.processEvents();window.grab().save('/tmp/release-connections.png')
    assert general.palette().color(QPalette.ColorRole.Window)==connections.palette().color(QPalette.ColorRole.Window)
    window.nav.setCurrentRow(5);window.tools_plan=plan_library(root);window.tools_tabs.setCurrentIndex(2);window.invalidate_tools_plan()
    window.tools_dj_only.setChecked(True);app.processEvents()
    assert sum(not window.tools_table.isRowHidden(i) for i in range(2))==1
    window.grab().save('/tmp/release-health.png')
    assert window.width()<=1280,window.width()
    assert 'All missing releases' in [window.timeline.itemText(i) for i in range(window.timeline.count())]
    window.nav.setCurrentRow(6);window.settings_tabs.setCurrentIndex(0)
    colours=[]
    for theme in ('Light','Dark'):
        window.appearance.setCurrentText(theme)
        loop=QEventLoop();QTimer.singleShot(100,loop.quit);loop.exec()
        assert general.palette().color(QPalette.ColorRole.Window)==connections.palette().color(QPalette.ColorRole.Window)
        colours.append(general.palette().color(QPalette.ColorRole.Window).lightness())
        window.grab().save('/tmp/release-settings-'+app.platformName()+'-'+theme.lower()+'.png')
    if app.platformName()=='cocoa':assert colours[0]>colours[1],colours
    window.close()
print('Unified settings surfaces, DJ-tag filter, release modes and window sizing passed.')
