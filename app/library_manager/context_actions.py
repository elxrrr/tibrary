"""Consistent, native contextual actions; no menu construction performs I/O."""
import json
import sys
from PySide6.QtWidgets import QMenu,QApplication,QDialog,QVBoxLayout,QPlainTextEdit,QDialogButtonBox


def metadata_dialog(parent,metadata):
    dialog=QDialog(parent);dialog.setWindowTitle('Metadata and evidence');dialog.resize(720,520)
    layout=QVBoxLayout(dialog);text=QPlainTextEdit();text.setReadOnly(True)
    text.setPlainText(json.dumps(metadata,indent=2,ensure_ascii=False,default=str));layout.addWidget(text)
    buttons=QDialogButtonBox(QDialogButtonBox.StandardButton.Close);buttons.rejected.connect(dialog.reject);layout.addWidget(buttons);dialog.exec()


def context_menu(parent,path=None,metadata=None,choose=None):
    from .ui import reveal_in_file_manager
    if isinstance(path,dict):path=path.get('path')
    menu=QMenu(parent)
    finder=menu.addAction('Show in Finder' if sys.platform=='darwin' else 'Show in file manager')
    finder.setEnabled(bool(path));finder.triggered.connect(lambda:reveal_in_file_manager(path))
    copy=menu.addAction('Copy path');copy.setEnabled(bool(path))
    copy.triggered.connect(lambda:QApplication.clipboard().setText(str(path)))
    view=menu.addAction('View metadata');view.setEnabled(metadata is not None)
    view.triggered.connect(lambda:metadata_dialog(parent,metadata))
    match=menu.addAction('Choose match for selected…');match.setEnabled(choose is not None)
    if choose:match.triggered.connect(choose)
    menu.addSeparator()
    return menu
