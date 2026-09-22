"""Small offscreen desktop check; never touches a real library or network."""
import sys
import tempfile
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from PySide6.QtWidgets import QApplication,QCheckBox
from PySide6.QtCore import Qt
from library_manager.ui import Window, STYLE
from library_manager.core import Store
from library_manager.demo import seed, catalogue

from ui_settings import isolate_settings
isolate_settings()
app = QApplication([])
app.setStyle('Fusion'); app.setStyleSheet(STYLE)
with tempfile.TemporaryDirectory() as folder:
    store = Store(Path(folder) / 'demo.sqlite3'); seed(store)
    window = Window(store, True); window.show(); app.processEvents()
    window.timeline.setCurrentText('All dates')
    assert window.coverage_table.rowCount() == 6
    window.artist_table.selectRow(0); window.local_details()
    assert window.local_table.rowCount() == 2
    window.filter.setCurrentText('Owned partial')
    assert window.coverage_table.rowCount() == 1
    window.filter.setCurrentText('All statuses')
    window.timeline.setCurrentText('Between newest two owned')
    assert window.coverage_table.rowCount() == 1
    window.timeline.setCurrentText('All dates')
    release = catalogue()['releases'][1]
    store.enqueue(release, release['tracks'][-1:]); window.refresh_queue()
    window.queue_table.cellWidget(0,0).findChild(QCheckBox).click()
    assert store.rows('SELECT approved FROM queue')[0]['approved'] == 1
    for index in range(7):
        window.nav.setCurrentRow(index); app.processEvents()
    window.nav.setCurrentRow(0); app.processEvents()
    window.grab().save('/tmp/tidal-overview.png')
    window.nav.setCurrentRow(3); app.processEvents()
    window.grab().save('/tmp/tidal-coverage.png')
    window.close()
print('Desktop smoke passed: seven screens, filters, local inspection, queue approval.')
