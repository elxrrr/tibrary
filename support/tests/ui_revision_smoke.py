import sys,json,tempfile,time
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication,QLineEdit,QCheckBox,QTableWidget,QStyleFactory
from PySide6.QtCore import QTimer
from library_manager.ui import Window
from library_manager.core import Store
from library_manager.maintenance import plan_library
from test_maintenance import fixture
from ui_settings import isolate_settings
isolate_settings();app=QApplication([])
if 'macOS' in QStyleFactory.keys():app.setStyle('macOS')
errors=[]
def fail(*args):
    errors.append(args);sys.__excepthook__(*args)
sys.excepthook=fail
with tempfile.TemporaryDirectory() as directory:
    store=Store(Path(directory)/'db');root=Path(directory)/'music';fixture(root/'Matt Fax'/'Belong - Mirth'/'song.flac')
    with store.connect() as db:db.execute('INSERT INTO roots VALUES(?,?,?)',(str(root),'today','complete'))
    release=dict(id='123',artist='A',title='Album',date='2020-01-01',type='ALBUM',available=True,quality='LOSSLESS',track_count=2,tracks_loaded=True,link_checked_at=time.time(),tracks=[dict(id='11',title='One',duration=123,isrc='GB0001'),dict(id='12',title='Two',duration=145,isrc='GB0002')])
    store.enqueue(release)
    window=Window(store);window.show();window.connection_dialog();app.processEvents()
    for key in ('client_id','client_secret'):
        field=window.connection_page_widget.findChild(QLineEdit,key)
        assert field.width()>=300 and field.height()>=40,(key,field.size())
    assert window.connection_page_widget.width()<=window.settings_tabs.widget(1).viewport().width(),(window.connection_page_widget.width(),window.settings_tabs.widget(1).viewport().width())
    window.grab().save('/tmp/connection-fixed.png')
    window.nav.setCurrentRow(4);app.processEvents()
    cell=window.queue_table.cellWidget(0,0);check=cell.findChild(QCheckBox)
    assert abs(check.geometry().center().x()-cell.rect().center().x())<=2
    assert window.queue_table.item(0,0).data(10) is None
    check.click();assert store.rows('SELECT approved FROM queue')[0]['approved']==1
    window.grab().save('/tmp/queue-fixed.png')
    def select_second():
        dialog=app.activeModalWidget();table=dialog.findChild(QTableWidget)
        assert table.columnCount()==6 and table.item(1,4).text()=='GB0002'
        table.clearSelection();table.selectRow(1);dialog.accept()
    QTimer.singleShot(0,select_second);window.queue_table.selectRow(0);window.edit_queue_tracks()
    row=store.rows('SELECT * FROM queue')[0];assert not row['approved']
    assert [t['id'] for t in json.loads(row['payload'])['selected_tracks']]==['12']
    window.tools_plan=plan_library(root);window.tools_snapshots[str(root)]=window.tools_plan
    window.nav.setCurrentRow(5)
    with patch('library_manager.maintenance.FLAC',side_effect=AssertionError('Rescan')):
        window.tools_tabs.setCurrentIndex(4);window.tools_dates.setChecked(False)
    assert window.tools_plan and 'date' not in window.tools_plan[0]['changes']
    assert window.nav.count()==7
    assert 'background:' not in app.styleSheet()
    window.tools_search.setText('does not exist');app.processEvents();assert window.tools_table.isRowHidden(0)
    window.tools_search.clear();app.processEvents();assert not window.tools_table.isRowHidden(0)
    window.grab().save('/tmp/tools-native.png')
    assert not errors,errors
    window.close()
print('Native-sized credentials, centred approvals, track subset editing and snapshot replanning passed.')
