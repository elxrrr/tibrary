import unittest,tempfile,copy
from pathlib import Path
from unittest.mock import patch
from PySide6.QtWidgets import QApplication,QMessageBox
from library_manager.core import Store,scan
from library_manager.maintenance import inspect_file,audio_digest
from library_manager.ui import Window
from test_maintenance import fixture
app=QApplication.instance() or QApplication([])

class ApplyWorkflowTests(unittest.TestCase):
    def test_full_tag_action_reconciles_original_library_after_navigation(self):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder).resolve();library=root/'music';file=library/'song.flac';fixture(file)
            store=Store(root/'db');scan(store,library)
            w=Window(store,demo_mode=True);w.demo_mode=False
            row=inspect_file(file,library);row.update(changes={'tracknumber':['01'],'tracktotal':['06']},target=str(file),operation='tags')
            w.tools_plan=[row];w.tools_snapshots[str(library)]=[row];before=audio_digest(file)
            captured=[]
            with patch.object(w,'tools_operation',return_value='tags'),patch.object(w,'tools_selected',return_value=[row]),patch.object(w,'job',side_effect=lambda op,done,**kw:captured.append((op,done))),patch.object(QMessageBox,'question',return_value=QMessageBox.StandardButton.Yes):
                w.tools_apply()
            self.assertEqual(len(captured),1)
            result=captured[0][0](lambda:False,lambda s:None)
            other=dict(row,path='other-library-file');w.tools_plan=[other]
            with patch.object(w.tools_root,'currentData',return_value='other-root'),patch.object(w,'invalidate_tools_plan'),patch.object(w,'_library_content_changed') as changed:
                captured[0][1](result)
            self.assertEqual(w.tools_plan,[other]);changed.assert_called_once_with(str(library))
            self.assertEqual(w.tools_snapshots[str(library)][0]['tags']['tracknumber'],['01'])
            self.assertEqual(audio_digest(file),before)
            w.close()
