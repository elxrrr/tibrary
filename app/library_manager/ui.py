from __future__ import annotations
import argparse
import copy
import queue
import json
import os
import re
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
import subprocess

from PySide6.QtCore import Qt, QThread, Signal, QSettings, QTimer, QItemSelectionModel, QItemSelection, Slot
from PySide6.QtGui import QDesktopServices, QPalette, QColor, QActionGroup, QAction, QFontDatabase, QKeySequence, QPen
from PySide6.QtCore import QUrl, QEvent, QRectF
from PySide6.QtWidgets import (QApplication, QMainWindow, QWidget, QHBoxLayout, QVBoxLayout, QLabel,
    QPushButton, QListWidget, QStackedWidget, QTableWidget, QTableWidgetItem, QHeaderView,
    QFileDialog, QMessageBox, QComboBox, QLineEdit, QDialog, QDialogButtonBox, QAbstractItemView, QDockWidget, QPlainTextEdit, QCheckBox, QFormLayout, QProgressBar, QSpinBox, QDoubleSpinBox, QScrollArea, QGridLayout, QInputDialog, QSizePolicy, QStyledItemDelegate, QStyle, QStyleOptionViewItem, QTabWidget, QFrame, QMenu, QGroupBox, QToolButton, QTreeWidget, QTreeWidgetItem)

from .core import Store, scan, resolve, coverage, import_legacy, now, norm, title_key
from .tidal import Tidal, CatalogueError, RequestPacer
from . import demo
from .credentials import Credentials, CredentialError
from .account import Account, REDIRECT
from .maintenance import plan_library, apply_plans, set_album_artist, validate_targets, normalized_date, replan, inspect_file
from .musical_keys import camelot_key
from .downloads import download_approved
from .matching import BatchMatcher
from .discovery import working_releases, verify_releases, needs_verification, release_groups
from .view_data import build_view, filter_coverage, release_date, release_is_out
from .virtual_table import VirtualTable

CHEVRON_ICON_PATH = str((Path(__file__).parent / 'icons' / 'chevron_down.png').resolve())

STYLE = """
QLabel#title { font-size:24px; font-weight:700; margin-bottom: 2px; }
QLabel#metric { padding:14px; font-size:22px; }
QLabel#muted { color: palette(placeholder-text); }

/* Modern rounded inputs and controls conforming to macOS HIG */
QLineEdit {
    border: 1px solid rgba(128, 128, 128, 0.28);
    border-radius: 8px;
    padding: 6px 10px;
    background-color: palette(base);
    color: palette(text);
    selection-background-color: #007AFF;
}
QLineEdit:focus {
    border: 1.5px solid #007AFF;
}

QComboBox {
    border: 1px solid rgba(128, 128, 128, 0.28);
    border-radius: 8px;
    padding: 5px 26px 5px 10px;
    background-color: palette(button);
    color: palette(button-text);
    min-height: 22px;
}
QComboBox:focus {
    border: 1.5px solid #007AFF;
}
QComboBox::drop-down {
    subcontrol-origin: padding;
    subcontrol-position: center right;
    width: 24px;
    border: none;
    background-color: transparent;
}
QComboBox::down-arrow {
    image: url("__CHEVRON_ICON_PATH__");
    width: 12px;
    height: 12px;
}
QComboBox QAbstractItemView {
    border: none;
    outline: none;
    background-color: palette(window);
    selection-background-color: #007AFF;
    selection-color: #FFFFFF;
    padding: 4px;
}
QComboBox QAbstractItemView::item {
    border: none;
    outline: none;
    padding: 6px 10px;
    border-radius: 6px;
    min-height: 20px;
}
QComboBox QAbstractItemView::item:hover,
QComboBox QAbstractItemView::item:selected {
    background-color: #007AFF;
    color: #FFFFFF;
}

QSpinBox, QDoubleSpinBox {
    border: 1px solid rgba(128, 128, 128, 0.28);
    border-radius: 8px;
    padding: 5px 24px 5px 8px;
    background-color: palette(base);
    color: palette(text);
}
QSpinBox:focus, QDoubleSpinBox:focus {
    border: 1.5px solid #007AFF;
}

QSpinBox::up-button, QDoubleSpinBox::up-button {
    subcontrol-origin: border; subcontrol-position: top right;
    width: 20px; height: 15px; margin-top: 2px; margin-right: 2px;
    border: none; border-top-right-radius: 6px;
}
QSpinBox::down-button, QDoubleSpinBox::down-button {
    subcontrol-origin: border; subcontrol-position: bottom right;
    width: 20px; height: 15px; margin-bottom: 2px; margin-right: 2px;
    border: none; border-bottom-right-radius: 6px;
}

QPushButton {
    border: 1px solid rgba(128, 128, 128, 0.28);
    border-radius: 8px;
    padding: 6px 14px;
    background-color: palette(button);
    color: palette(button-text);
    font-weight: 500;
}
QPushButton:hover {
    background-color: rgba(255, 255, 255, 0.14);
    border-color: rgba(0, 122, 255, 0.65);
}
QPushButton:pressed {
    background-color: rgba(0, 0, 0, 0.15);
}
QPushButton:disabled {
    color: rgba(140, 140, 145, 0.5);
    background-color: rgba(128, 128, 128, 0.08);
    border-color: rgba(128, 128, 128, 0.15);
}
QPushButton#primary {
    background-color: #007AFF;
    color: #FFFFFF;
    border: 1px solid #0062D2;
    font-weight: 600;
}
QPushButton#primary:hover {
    background-color: #1A8CFF;
    border-color: #40A0FF;
}
QPushButton#primary:pressed {
    background-color: #005BB5;
}
QPushButton#primary:disabled {
    background-color: rgba(128, 128, 128, 0.12);
    color: rgba(140, 140, 145, 0.45);
    border: 1px solid rgba(128, 128, 128, 0.18);
}

QPushButton#operation_card {
    text-align: left;
    padding: 11px 13px;
    min-height: 48px;
    font-size: 12px;
    font-weight: 600;
    background-color: rgba(128, 128, 128, 0.08);
}
QPushButton#operation_card:hover {
    background-color: rgba(0, 122, 255, 0.10);
    border-color: rgba(0, 122, 255, 0.65);
}
QPushButton#operation_card:checked {
    background-color: rgba(0, 122, 255, 0.20);
    border: 2px solid #007AFF;
    color: palette(text);
}

QMenu {
    background-color: palette(window);
    color: palette(window-text);
    border: 1px solid rgba(128, 128, 128, 0.28);
    border-radius: 8px;
    padding: 5px;
}
QMenu::item {
    padding: 6px 28px 6px 24px;
    border-radius: 6px;
}
QMenu::item:selected {
    background-color: #007AFF;
    color: #FFFFFF;
}
QMenu::item:disabled { color: palette(placeholder-text); }
QMenu::separator { height: 1px; background: rgba(128, 128, 128, 0.25); margin: 5px 8px; }

QListWidget, QTreeWidget {
    border: 1px solid rgba(128, 128, 128, 0.18);
    border-radius: 8px;
    padding: 6px 4px;
    background-color: palette(base);
}
QListWidget::item, QTreeWidget::item {
    padding: 7px 10px;
    border-radius: 6px;
    margin: 1px 2px;
    font-size: 13px;
    font-weight: 500;
}
QListWidget::item:hover, QTreeWidget::item:hover {
    background-color: rgba(128, 128, 128, 0.08);
}
QListWidget::item:selected, QTreeWidget::item:selected {
    background-color: #007AFF;
    color: #FFFFFF;
    font-weight: 600;
}

QTreeWidget::item {
    margin: 0;
    border-radius: 0;
    padding: 7px 6px;
}
QTreeWidget::item:selected {
    background-color: palette(highlight);
    color: palette(highlighted-text);
}

QTableWidget, QTableView {
    border: 1px solid rgba(128, 128, 128, 0.18);
    border-radius: 8px;
    background-color: palette(base);
    gridline-color: rgba(128, 128, 128, 0.15);
}
QTableWidget::item, QTableView::item {
    padding: 6px 8px;
}
QHeaderView {
    background-color: transparent;
    border-top-left-radius: 7px;
    border-top-right-radius: 7px;
    border: none;
}
QHeaderView::section {
    padding: 6px 8px;
    font-weight: 600;
    font-size: 11px;
}
QHeaderView::section:first {
    border-top-left-radius: 7px;
}
QHeaderView::section:last {
    border-top-right-radius: 7px;
}
QTableCornerButton::section {
    border-top-left-radius: 7px;
}
QPlainTextEdit {
    border: 1px solid rgba(128, 128, 128, 0.2);
    border-radius: 8px;
    padding: 8px;
}
QTabWidget::pane {
    border: none;
    top: 0px;
    padding: 0px;
}
QTabBar {
    qproperty-drawBase: 0;
    background-color: __TAB_BG__;
    border: 1px solid __TAB_BORDER__;
    border-radius: 7px;
    padding: 2px;
    margin-bottom: 4px;
}
QTabBar::tab {
    padding: 5px 14px;
    border-radius: 5px;
    margin: 0px;
    border: none;
    color: __TAB_COLOR__;
    font-size: 12px;
    font-weight: 500;
    background-color: transparent;
}
QTabBar::tab:hover:!selected {
    background-color: __TAB_HOVER_BG__;
}
QTabBar::tab:selected {
    background-color: #007AFF;
    color: #FFFFFF;
    font-weight: 600;
}
QGroupBox {
    border: 1px solid rgba(128, 128, 128, 0.22);
    border-radius: 8px;
    margin-top: 22px;
    padding-top: 14px;
    background-color: rgba(128, 128, 128, 0.04);
}
QGroupBox::title {
    subcontrol-origin: margin;
    subcontrol-position: top left;
    left: 16px;
    padding: 0 6px;
    font-weight: 600;
    font-size: 12px;
    color: palette(window-text);
}
"""
STYLE += '\n'.join(
    f'QSpinBox::{direction}-arrow, QDoubleSpinBox::{direction}-arrow {{ image: url("{(Path(__file__).parent / "assets" / f"spin-{direction}.svg").as_posix()}"); width: 12px; height: 10px; }}'
    for direction in ('up','down')
)
LIGHT_STYLE = STYLE


def reveal_in_file_manager(path_str):
    """Reveal a local path without decoding literal percent signs or blocking Qt."""
    if not path_str:return False
    if isinstance(path_str,dict):path_str=path_str.get('path')
    if not path_str:return False
    text=str(path_str)
    if text.lower().startswith('file:'):
        url=QUrl(text)
        if not url.isLocalFile():return False
        text=url.toLocalFile()
    p=Path(text).expanduser().resolve()
    # Missing files can still reveal their nearest existing containing folder.
    while not p.exists() and p!=p.parent:p=p.parent
    if not p.exists():return False
    try:
        if sys.platform=='darwin':subprocess.Popen(['open','-R',str(p)] if p.is_file() else ['open',str(p)])
        elif sys.platform.startswith('win'):subprocess.Popen(['explorer','/select,',str(p)] if p.is_file() else ['explorer',str(p)])
        else:return QDesktopServices.openUrl(QUrl.fromLocalFile(str(p.parent if p.is_file() else p)))
        return True
    except OSError:return False


def parse_utc_or_iso(ts):
    if not ts:
        return None
    if isinstance(ts, (int, float)):
        return datetime.fromtimestamp(ts, tz=timezone.utc).astimezone()
    s = str(ts).strip()
    if not s or s in ('—', 'Never', 'None'):
        return None
    if s.endswith('Z'):
        s = s[:-1] + '+00:00'
    try:
        dt = datetime.fromisoformat(s)
        if dt.tzinfo is None:
            dt = dt.replace(tzinfo=timezone.utc)
        return dt.astimezone()
    except Exception:
        try:
            dt = datetime.strptime(s[:19], '%Y-%m-%d %H:%M:%S').replace(tzinfo=timezone.utc)
            return dt.astimezone()
        except Exception:
            return None


def format_user_datetime(ts, include_seconds=False, date_only=False, time_only=False):
    dt = parse_utc_or_iso(ts)
    if not dt:
        return str(ts) if ts else '—'
    if date_only:
        return dt.strftime('%Y-%m-%d')
    if time_only:
        return dt.strftime('%H:%M:%S' if include_seconds else '%H:%M')
    fmt = '%Y-%m-%d %H:%M:%S' if include_seconds else '%Y-%m-%d %H:%M'
    return dt.strftime(fmt)


class Worker(QThread):
    message = Signal(str)
    result = Signal(object)
    record_saved = Signal(object)
    records_saved = Signal(object)
    failure = Signal(str)

    def __init__(self, operation):
        super().__init__()
        self.operation = operation

    def run(self):
        last_progress=0.0
        def report(message):
            nonlocal last_progress
            routine=message in ('applied','unchanged') or message.startswith(('Checking album tags ·','Applying file ','Checking folder ·','Reading audio tags ·','Scanned ','Checked ','Checking library health ·','Fetching ','Catalogue request ·','Using cached release summaries ·','Checking track ','Linked (','Needs review ('))
            current=time.monotonic()
            if routine and current-last_progress<.2:return
            if routine:last_progress=current
            self.message.emit(message)
        try:
            self.result.emit(self.operation(self.isInterruptionRequested, report))
        except (CatalogueError, CredentialError) as exc:
            self.failure.emit(str(exc))
        except Exception as exc:
            from .diagnostics import record_failure
            record_failure('Background operation',exc)
            self.failure.emit('Operation failed. Saved data is retained. Diagnostic details are in the app’s logs folder.')


class QuietFocusDelegate(QStyledItemDelegate):
    """Draw row checkboxes explicitly: native macOS delegates can lose them in styled tables."""
    def paint(self,painter,option,index):
        option=QStyleOptionViewItem(option)
        self.initStyleOption(option,index)
        option.state &= ~QStyle.StateFlag.State_HasFocus
        state=index.data(Qt.ItemDataRole.CheckStateRole)
        if state is None:
            super().paint(painter,option,index);return
        option.features &= ~QStyleOptionViewItem.ViewItemFeature.HasCheckIndicator
        style=option.widget.style() if option.widget else QApplication.style()
        style.drawControl(QStyle.ControlElement.CE_ItemViewItem,option,painter,option.widget)
        rect=QRectF(option.rect.center().x()-8,option.rect.center().y()-8,16,16)
        checked=state in (Qt.CheckState.Checked,Qt.CheckState.Checked.value)
        partial=state in (Qt.CheckState.PartiallyChecked,Qt.CheckState.PartiallyChecked.value)
        painter.save();painter.setRenderHint(painter.RenderHint.Antialiasing)
        ink=option.palette.color(QPalette.ColorRole.Text)
        painter.setPen(QPen(ink,1.2));painter.setBrush(option.palette.base());painter.drawRoundedRect(rect,3,3)
        painter.setPen(QPen(ink,2,Qt.PenStyle.SolidLine,Qt.PenCapStyle.RoundCap))
        x,y=rect.x(),rect.y()
        if partial:painter.drawLine(int(x+4),int(y+8),int(x+12),int(y+8))
        elif checked:
            painter.drawLine(int(x+3),int(y+8),int(x+7),int(y+12));painter.drawLine(int(x+7),int(y+12),int(x+13),int(y+4))
        painter.restore()

    def editorEvent(self,event,model,option,index):
        state=index.data(Qt.ItemDataRole.CheckStateRole)
        if state is None:return super().editorEvent(event,model,option,index)
        if not index.flags() & Qt.ItemFlag.ItemIsEnabled:return False
        activate=(event.type()==QEvent.Type.MouseButtonRelease and event.button()==Qt.MouseButton.LeftButton and option.rect.contains(event.position().toPoint())) or (event.type()==QEvent.Type.KeyPress and event.key() in (Qt.Key.Key_Space,Qt.Key.Key_Select))
        if activate:
            checked=state in (Qt.CheckState.Checked,Qt.CheckState.Checked.value)
            return model.setData(index,Qt.CheckState.Unchecked if checked else Qt.CheckState.Checked,Qt.ItemDataRole.CheckStateRole)
        return False


DASHBOARD_CARD_STYLE = '''
            QFrame#overview_card {
                background-color: rgba(128, 128, 128, 0.08);
                border: 1px solid rgba(128, 128, 128, 0.18);
                border-radius: 10px;
            }
            QLabel#card_title {
                font-size: 11px;
                font-weight: 700;
                text-transform: uppercase;
                letter-spacing: 0.5px;
            }
            QLabel#card_metric {
                font-size: 19px;
                font-weight: 700;
            }
            QLabel#card_sub {
                font-size: 12px;
            }
            QPushButton#card_btn {
                font-size: 12px;
                font-weight: 500;
                padding: 4px 10px;
                border-radius: 6px;
            }
        '''


class ActionCard(QFrame):
    """A keyboard-accessible dashboard card whose complete surface opens its view."""
    activated = Signal()
    def __init__(self,parent=None):
        super().__init__(parent);self.setCursor(Qt.CursorShape.PointingHandCursor)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)
    def mouseReleaseEvent(self,event):
        if event.button()==Qt.MouseButton.LeftButton and self.rect().contains(event.position().toPoint()):
            self.activated.emit();event.accept();return
        super().mouseReleaseEvent(event)
    def keyPressEvent(self,event):
        if event.key() in (Qt.Key.Key_Return,Qt.Key.Key_Enter,Qt.Key.Key_Space):
            self.activated.emit();event.accept();return
        super().keyPressEvent(event)


class HealthCard(ActionCard):
    def __init__(self,title,description):
        super().__init__();self.setObjectName('overview_card');self.setStyleSheet(DASHBOARD_CARD_STYLE);self.setMinimumHeight(90)
        layout=QVBoxLayout(self);layout.setContentsMargins(14,12,14,12);layout.setSpacing(4)
        self.title_label=QLabel(title);self.title_label.setObjectName('card_title')
        self.metric_label=QLabel('Inspect first');self.metric_label.setObjectName('card_metric')
        self.description_label=QLabel(description);self.description_label.setObjectName('card_sub');self.description_label.setWordWrap(True)
        for label in (self.title_label,self.metric_label,self.description_label):
            label.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents);layout.addWidget(label)
    def setText(self,text):
        self.metric_label.setText(text.split('\n',1)[-1])
    def text(self):return self.title_label.text()+'\n'+self.metric_label.text()


class OperationCard(QPushButton):
    """A clear single-operation selector with a live affected-file count."""
    def __init__(self,title,description='',parent=None):
        super().__init__(parent);self.title=title;self.description=description
        self.setObjectName('operation_card');self.setCheckable(True)
        self.setSizePolicy(QSizePolicy.Policy.Expanding,QSizePolicy.Policy.Preferred)
        self.setToolTip(description);self.set_count(None)
    def set_count(self,count):
        detail='Inspect to calculate' if count is None else ('No files affected' if count==0 else f'{count:,} file'+('' if count==1 else 's')+' affected')
        self.setText(f'{self.title}\n{detail}')


class SidebarDelegate(QuietFocusDelegate):
    """Native selection with a fixed emoji column and eight-point text gap."""
    def paint(self,painter,option,index):
        opt=QStyleOptionViewItem(option);self.initStyleOption(opt,index)
        raw=opt.text;parts=raw.split(maxsplit=1)
        emoji,label=(parts[0],parts[1]) if len(parts)==2 else ('•',raw)
        opt.text='';opt.state &= ~QStyle.StateFlag.State_HasFocus
        style=opt.widget.style() if opt.widget else QApplication.style()
        style.drawControl(QStyle.ControlElement.CE_ItemViewItem,opt,painter,opt.widget)
        rect=style.subElementRect(QStyle.SubElement.SE_ItemViewItemText,opt,opt.widget)
        painter.save();painter.setFont(opt.font)
        selected=bool(opt.state & QStyle.StateFlag.State_Selected)
        painter.setPen(opt.palette.color(QPalette.ColorRole.HighlightedText if selected else QPalette.ColorRole.Text))
        glyph=rect.adjusted(4,0,0,0);glyph.setWidth(20)
        painter.drawText(glyph,Qt.AlignmentFlag.AlignCenter,emoji)
        text_rect=rect.adjusted(32,0,-4,0)  # 4px inset + 20px emoji + 8px gap.
        painter.drawText(text_rect,Qt.AlignmentFlag.AlignVCenter|Qt.AlignmentFlag.AlignLeft,
                         opt.fontMetrics.elidedText(label,Qt.TextElideMode.ElideRight,text_rect.width()))
        painter.restore()


class WorkflowTabs(QTabWidget):
    """Size the operation header dynamically, leaving maximum room for the file list."""
    def sizeHint(self):
        w = self.currentWidget()
        if w and w.layout():
            h = w.layout().sizeHint().height()
            tb_h = 0 if self.tabBar().isHidden() else self.tabBar().sizeHint().height()
            from PySide6.QtCore import QSize
            return QSize(super().sizeHint().width(), h + tb_h + 8)
        return super().sizeHint()

    def minimumSizeHint(self):
        return self.sizeHint()

    def fit_current_page(self):
        page=self.currentWidget()
        if page and page.layout():
            width=max(200,self.width()-8)
            body=page.layout()
            height=body.totalHeightForWidth(width) if body.hasHeightForWidth() else body.sizeHint().height()
            self.setFixedHeight(max(40,height)+8)

    def resizeEvent(self,event):
        super().resizeEvent(event);self.fit_current_page()

    def setCurrentIndex(self, index):
        if index == 5 and self.count() <= 5:
            win = self.window()
            if hasattr(win, 'nav'):
                win.nav.setCurrentRow(2)
                return
        super().setCurrentIndex(index)
        self.fit_current_page();self.updateGeometry()


class StartupOverlay(QWidget):
    """Initialisation loading screen shown on launch while verifying library tags."""
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setObjectName("startup_overlay")
        self.setStyleSheet("""
            #startup_overlay {
                background-color: rgba(18, 18, 22, 0.78);
            }
            QFrame#startup_card {
                background-color: palette(window);
                border: 1px solid palette(mid);
                border-radius: 12px;
            }
        """)
        layout = QVBoxLayout(self)
        layout.setAlignment(Qt.AlignmentFlag.AlignCenter)

        card = QFrame()
        card.setObjectName("startup_card")
        card.setFixedWidth(380)
        card_layout = QVBoxLayout(card)
        card_layout.setContentsMargins(28, 24, 28, 24)
        card_layout.setSpacing(12)
        card_layout.setAlignment(Qt.AlignmentFlag.AlignCenter)

        title = QLabel("Initialising Library")
        font = title.font(); font.setPointSize(16); font.setBold(True)
        title.setFont(font)
        title.setAlignment(Qt.AlignmentFlag.AlignCenter)
        card_layout.addWidget(title)

        self.status_label = QLabel("Verifying local tags & library status…")
        self.status_label.setWordWrap(True)
        self.status_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.status_label.setObjectName("muted")
        card_layout.addWidget(self.status_label)

        self.progress_bar = QProgressBar()
        self.progress_bar.setFixedHeight(6)
        self.progress_bar.setTextVisible(False)
        self.progress_bar.setRange(0, 0)
        card_layout.addWidget(self.progress_bar)

        layout.addWidget(card)
        self.hide()

    def resizeToParent(self):
        if self.parent():
            self.resize(self.parent().size())


class ClosingOverlay(QWidget):
    """Interstitial screen shown on window close / Cmd+Q while safely stopping background tasks."""
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setAttribute(Qt.WidgetAttribute.WA_StyledBackground, True)
        self.setObjectName("closing_overlay")
        self.setStyleSheet("""
            #closing_overlay {
                background-color: rgba(18, 18, 22, 0.78);
            }
            QFrame#closing_card {
                background-color: palette(window);
                border: 1px solid palette(mid);
                border-radius: 12px;
            }
        """)
        layout = QVBoxLayout(self)
        layout.setAlignment(Qt.AlignmentFlag.AlignCenter)

        card = QFrame()
        card.setObjectName("closing_card")
        card.setFixedWidth(380)
        card_layout = QVBoxLayout(card)
        card_layout.setContentsMargins(28, 24, 28, 24)
        card_layout.setSpacing(12)
        card_layout.setAlignment(Qt.AlignmentFlag.AlignCenter)

        title = QLabel("Closing Tibrary")
        font = title.font(); font.setPointSize(16); font.setBold(True)
        title.setFont(font)
        title.setAlignment(Qt.AlignmentFlag.AlignCenter)
        card_layout.addWidget(title)

        self.status_label = QLabel("Finishing in-flight operations and saving cache safely…")
        self.status_label.setWordWrap(True)
        self.status_label.setAlignment(Qt.AlignmentFlag.AlignCenter)
        self.status_label.setObjectName("muted")
        card_layout.addWidget(self.status_label)

        self.progress_bar = QProgressBar()
        self.progress_bar.setFixedHeight(6)
        self.progress_bar.setTextVisible(False)
        self.progress_bar.setRange(0, 0)
        card_layout.addWidget(self.progress_bar)

        layout.addWidget(card)
        self.hide()

    def resizeToParent(self):
        if self.parent():
            self.resize(self.parent().size())


def format_release_type(raw_type):
    if not raw_type:
        return 'Unknown'
    t = str(raw_type).strip()
    upper = t.upper()
    if upper == 'SINGLE':
        return 'Single'
    elif upper == 'ALBUM':
        return 'Album'
    elif upper == 'EP':
        return 'EP'
    elif upper == 'COMPILATION':
        return 'Compilation'
    elif upper == 'TRACK':
        return 'Track'
    elif upper == 'PODCAST':
        return 'Podcast'
    return t.title() if t.isupper() else t


class SortableTableItem(QTableWidgetItem):
    def __lt__(self,other):
        def key(text):
            value=text.strip().replace(',','')
            try:return (0,float(value))
            except ValueError:return (1,text.casefold())
        return key(self.text())<key(other.text())


def table(headers, virtual=False):
    widget = VirtualTable(headers) if virtual else QTableWidget(0, len(headers))
    widget.setItemDelegate(QuietFocusDelegate(widget))
    headers=['Release' if h=='Album' else h for h in headers]
    if isinstance(widget,VirtualTable):widget.model().headers=headers
    else:widget.setHorizontalHeaderLabels(headers)
    widget.verticalHeader().hide()
    widget.verticalHeader().setDefaultSectionSize(30)
    widget.setAlternatingRowColors(True)
    widget.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
    widget.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
    widget.setWordWrap(False)
    header = widget.horizontalHeader()
    header.setStretchLastSection(True)
    header.setMinimumSectionSize(60)
    for col, h in enumerate(headers):
        header.setSectionResizeMode(col, QHeaderView.ResizeMode.Interactive)
        h_lower = h.lower()
        if 'approve' in h_lower:
            widget.setColumnWidth(col, 75)
        elif 'date' in h_lower or 'year' in h_lower:
            widget.setColumnWidth(col, 125)
        elif 'time' in h_lower:
            widget.setColumnWidth(col, 95)
        elif 'type' in h_lower:
            widget.setColumnWidth(col, 95)
        elif 'track' in h_lower or 'disc' in h_lower:
            widget.setColumnWidth(col, 80)
        elif 'coverage' in h_lower or 'status' in h_lower:
            widget.setColumnWidth(col, 140)
        elif 'quality' in h_lower:
            widget.setColumnWidth(col, 130)
        elif 'source' in h_lower or 'availability' in h_lower or 'selection' in h_lower:
            widget.setColumnWidth(col, 140)
        elif 'destination' in h_lower or 'changes' in h_lower:
            widget.setColumnWidth(col, 220)
        elif 'result' in h_lower or 'attention' in h_lower:
            widget.setColumnWidth(col, 180)
        elif 'artist' in h_lower:
            widget.setColumnWidth(col, 240)
        elif 'release' in h_lower or 'album' in h_lower:
            widget.setColumnWidth(col, 280)
        elif 'file' in h_lower:
            widget.setColumnWidth(col, 280)
        elif 'summary' in h_lower:
            widget.setColumnWidth(col, 280)
        else:
            widget.setColumnWidth(col, 150)
    if not virtual:
        header.setSortIndicator(-1,Qt.SortOrder.AscendingOrder);widget.setSortingEnabled(True)
    return widget


def source_row(widget,row):
    cell=widget.item(row,0)
    original=cell.data(Qt.ItemDataRole.UserRole+1) if cell else None
    return original if isinstance(original,int) else row


def select_rows(widget,indices):
    selection=QItemSelection();start=last=None
    for index in indices:
        if last is not None and index!=last+1:
            selection.select(widget.model().index(start,0),widget.model().index(last,widget.columnCount()-1));start=None
        if start is None:start=index
        last=index
    if start is not None:selection.select(widget.model().index(start,0),widget.model().index(last,widget.columnCount()-1))
    widget.selectionModel().select(selection,QItemSelectionModel.SelectionFlag.ClearAndSelect|QItemSelectionModel.SelectionFlag.Rows)


def fill(widget, rows, keys=None, row_types=None):
    def row_key(row):
        first=widget.item(row,0)
        if first and first.data(Qt.ItemDataRole.UserRole) is not None: return (first.data(Qt.ItemDataRole.UserRole),)
        if first and first.text(): return (first.text(),)
        return tuple(widget.item(row,c).text() if widget.item(row,c) else '' for c in range(1,min(3,widget.columnCount())))
    selected_keys={row_key(index.row()) for index in widget.selectionModel().selectedRows()}
    scroll = widget.verticalScrollBar().value()
    blocked=widget.blockSignals(True);widget.setUpdatesEnabled(False)
    try:
        if isinstance(widget,VirtualTable):widget.model().replace(rows,keys,row_types=row_types)
        else:
            sorting=widget.isSortingEnabled()
            widget.setSortingEnabled(False)
            widget.setRowCount(len(rows))
            for i,row in enumerate(rows):
                is_ignored = bool(row_types and row_types.get(i) == 'ignored')
                for j,value in enumerate(row):
                    item=SortableTableItem(str(value));item.setToolTip(str(value))
                    if is_ignored:
                        item.setForeground(QColor(135, 135, 140))
                    if j==0:item.setData(Qt.ItemDataRole.UserRole+1,i)
                    if j==0 and keys is not None:item.setData(Qt.ItemDataRole.UserRole,keys[i])
                    widget.setItem(i,j,item)
            widget.verticalHeader().setDefaultSectionSize(30)
            if sorting:widget.setSortingEnabled(True)
        widget.clearSelection()
        if selected_keys:select_rows(widget,(i for i in range(widget.rowCount()) if row_key(i) in selected_keys))
        widget.verticalScrollBar().setValue(scroll)
    finally:
        widget.setUpdatesEnabled(True);widget.blockSignals(blocked)


class TidalFavouritesDialog(QDialog):
    def __init__(self, parent, favourites):
        super().__init__(parent)
        self.parent_window = parent
        self.favourites = list(favourites or [])
        self.setWindowTitle('Online Favourites Comparison')
        self.resize(820, 540)
        root = QVBoxLayout(self)
        root.setContentsMargins(16, 16, 16, 16)
        root.setSpacing(12)

        header = QLabel('Compare your local album artists against your liked online artists.')
        header.setObjectName('muted')
        root.addWidget(header)

        local_artists_dict = {}
        for row in getattr(parent, 'artist_rows', []):
            local_artists_dict[row[0].strip()] = row[1]
        if not local_artists_dict and getattr(parent, '_view_data', None):
            local_artists_dict = {a: len(t) for a, t in parent._view_data.get('artists', {}).items()}
        if not local_artists_dict and hasattr(parent, 'store'):
            try:
                local_artists_dict = {a: len(t) for a, t in parent.store.artists().items()}
            except Exception:
                pass

        mappings = {r['artist']: r['tidal_id'] for r in parent.store.rows('SELECT artist, tidal_id FROM mappings WHERE tidal_id IS NOT NULL')}
        from .matching import norm
        local_by_norm = {norm(name): (name, count) for name, count in local_artists_dict.items()}
        mapped_tidal_ids = set(mappings.values())

        self.missing_locally = []
        self.in_both = []
        self.local_only = []

        fav_ids = set()
        fav_names = set()
        for fav in self.favourites:
            fid = str(fav.get('id', ''))
            fname = fav.get('name', '')
            fav_ids.add(fid)
            fav_names.add(norm(fname))
            matched_local = None
            if fid in mapped_tidal_ids:
                for l_name, tid in mappings.items():
                    if tid == fid and l_name in local_artists_dict:
                        matched_local = (l_name, local_artists_dict[l_name])
                        break
            if not matched_local and norm(fname) in local_by_norm:
                matched_local = local_by_norm[norm(fname)]

            if matched_local:
                self.in_both.append((matched_local[0], matched_local[1], fid or '—'))
            else:
                self.missing_locally.append((fname, fid))

        for l_name, l_count in local_artists_dict.items():
            tid = mappings.get(l_name, '')
            if tid and tid in fav_ids:
                continue
            if norm(l_name) in fav_names:
                continue
            self.local_only.append((l_name, l_count, tid or 'Unlinked'))

        self.displayed_local_only = list(self.local_only)

        self.tabs = QTabWidget()
        self.tabs.setDocumentMode(False)

        # Tab 1: Missing locally
        t1 = QWidget(); l1 = QVBoxLayout(t1); l1.setContentsMargins(12, 12, 12, 12)
        l1.addWidget(QLabel('Liked artists that have no files in your local library:'))
        self.missing_tbl = table(['Artist name', 'Online ID'])
        self.missing_tbl.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.missing_tbl.itemDoubleClicked.connect(lambda item: self.open_selected_in_tidal())
        fill(self.missing_tbl, [(a[0], a[1]) for a in self.missing_locally])
        l1.addWidget(self.missing_tbl)
        row1 = QHBoxLayout()
        open_tidal_btn = QPushButton('Open on web')
        open_tidal_btn.clicked.connect(self.open_selected_in_tidal)
        row1.addWidget(open_tidal_btn)
        scan_btn = QPushButton('Scan discography for selected')
        scan_btn.clicked.connect(self.scan_missing_discography)
        row1.addWidget(scan_btn)
        row1.addStretch()
        l1.addLayout(row1)
        self.tabs.addTab(t1, f'Missing locally ({len(self.missing_locally)})')

        # Tab 2: In local & online
        t2 = QWidget(); l2 = QVBoxLayout(t2); l2.setContentsMargins(12, 12, 12, 12)
        l2.addWidget(QLabel('Artists in your local library that are also in your online favourites:'))
        self.both_tbl = table(['Album artist', 'Local tracks', 'Online ID'])
        fill(self.both_tbl, [(a[0], str(a[1]), a[2]) for a in self.in_both])
        l2.addWidget(self.both_tbl)
        self.tabs.addTab(t2, f'In local && online ({len(self.in_both)})')

        # Tab 3: Local only
        t3 = QWidget(); l3 = QVBoxLayout(t3); l3.setContentsMargins(12, 12, 12, 12)
        l3.addWidget(QLabel('Artists in your local library that are not yet liked in your online favourites:'))
        filter_bar = QHBoxLayout()
        filter_bar.addWidget(QLabel('Filter:'))
        self.local_filter = QComboBox()
        self.local_filter.addItems(['All artists', 'Linked only', 'Unlinked only'])
        self.local_filter.currentTextChanged.connect(self.apply_local_filter)
        filter_bar.addWidget(self.local_filter)
        filter_bar.addStretch()
        l3.addLayout(filter_bar)

        self.local_tbl = table(['Album artist', 'Local tracks', 'Online ID'])
        self.local_tbl.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        fill(self.local_tbl, [(a[0], str(a[1]), a[2]) for a in self.displayed_local_only])
        l3.addWidget(self.local_tbl)
        row3 = QHBoxLayout()
        add_btn = QPushButton('Add selected to favourites')
        add_btn.setObjectName('primary')
        add_btn.clicked.connect(self.add_selected_to_favourites)
        row3.addWidget(add_btn)
        row3.addStretch()
        l3.addLayout(row3)
        self.tabs.addTab(t3, f'Local only ({len(self.local_only)})')

        root.addWidget(self.tabs)
        close_box = QHBoxLayout()
        close_box.addStretch()
        close_btn = QPushButton('Close')
        close_btn.clicked.connect(self.accept)
        close_box.addWidget(close_btn)
        root.addLayout(close_box)

    def open_selected_in_tidal(self):
        indexes = self.missing_tbl.selectionModel().selectedRows()
        if not indexes:
            return
        idx = source_row(self.missing_tbl,indexes[0].row())
        if 0 <= idx < len(self.missing_locally):
            tid = self.missing_locally[idx][1]
            if tid and tid != '—':
                QDesktopServices.openUrl(QUrl(f"https://listen.tidal.com/artist/{tid}"))

    def apply_local_filter(self):
        mode = self.local_filter.currentText()
        if mode == 'Linked only':
            self.displayed_local_only = [x for x in self.local_only if x[2] and x[2] != 'Unlinked']
        elif mode == 'Unlinked only':
            self.displayed_local_only = [x for x in self.local_only if not x[2] or x[2] == 'Unlinked']
        else:
            self.displayed_local_only = list(self.local_only)
        fill(self.local_tbl, [(a[0], str(a[1]), a[2]) for a in self.displayed_local_only])

    def scan_missing_discography(self):
        indexes = self.missing_tbl.selectionModel().selectedRows()
        if not indexes:
            QMessageBox.information(self, 'Selection required', 'Select one or more artists to scan.')
            return
        selected = [self.missing_locally[source_row(self.missing_tbl,idx.row())] for idx in indexes]
        self.accept()
        self.parent_window.nav.setCurrentRow(3)
        self.parent_window.log(f'Selected {len(selected)} missing artist(s) from online favourites for discography review.')

    def add_selected_to_favourites(self):
        indexes = self.local_tbl.selectionModel().selectedRows()
        if not indexes:
            QMessageBox.information(self, 'Selection required', 'Select one or more artists to add to favourites.')
            return
        selected = [self.displayed_local_only[source_row(self.local_tbl,idx.row())] for idx in indexes]
        valid = [item for item in selected if item[2] and item[2] != 'Unlinked']
        if not valid:
            QMessageBox.information(self, 'Link required', 'Selected artists must be linked first.')
            return

        def update_ui_after_add(added_items):
            added_set = {item[0] for item in added_items}
            self.local_only = [x for x in self.local_only if x[0] not in added_set]
            for item in added_items:
                self.in_both.append(item)
                self.favourites.append({'id': item[2], 'name': item[0]})
            fill(self.both_tbl, [(a[0], str(a[1]), a[2]) for a in self.in_both])
            self.apply_local_filter()
            self.tabs.setTabText(1, f'In local && online ({len(self.in_both)})')
            self.tabs.setTabText(2, f'Local only ({len(self.local_only)})')
            if hasattr(self.parent_window, 'cached_tidal_favourites'):
                self.parent_window.cached_tidal_favourites = list(self.favourites)
            if hasattr(self.parent_window, 'card_favs_metric'):
                self.parent_window.card_favs_metric.setText(f'{len(self.favourites):,} favourites')

        if self.parent_window.demo_mode:
            update_ui_after_add(valid)
            QMessageBox.information(self, 'Favourites updated', f'Added {len(valid)} artist(s) to favourites (demo simulation).')
            return

        def work(cancel, progress):
            api = self.parent_window.api(cancel, progress)
            added = 0
            for name, tracks, tid in valid:
                if cancel(): break
                progress(f'Adding {name} (ID: {tid}) to favourites…')
                api.add_favourite_artist(tid)
                added += 1
            return added

        def done(added_count):
            update_ui_after_add(valid[:added_count])
            msg = f'Successfully added {added_count} artist(s) to favourites.'
            QMessageBox.information(self, 'Favourites updated', msg)
            self.parent_window.log(msg)

        self.parent_window.job(work, done, label='Add artists to favourites')


class ExtendedArtistReviewDialog(QDialog):
    """Dialog showing online releases discovered via ISRC / recording search for mis-tagged artists."""
    def __init__(self, parent, artist_name, local_tracks, discoveries):
        super().__init__(parent)
        self.chosen_discovery = None
        self.artist_name = artist_name
        self.local_tracks = list(local_tracks or [])
        self.discoveries = list(discoveries or [])
        self.setWindowTitle(f'Extended Review · Discovered Releases for “{artist_name}”')
        self.resize(780, 520)

        layout = QVBoxLayout(self)
        layout.setSpacing(12)
        layout.setContentsMargins(18, 16, 18, 16)

        heading = QLabel(f'Discovered Online Releases for “{artist_name}”')
        font = heading.font(); font.setPointSize(15); font.setBold(True); heading.setFont(font)
        layout.addWidget(heading)

        desc = QLabel(
            f'Local tracks under <b>{artist_name}</b> were cross-referenced against the Online API and catalogue by recording ISRC and title.<br>'
            'If a track was mis-tagged with the wrong album artist, you can retag and move it into the proper artist directory.'
        )
        desc.setWordWrap(True)
        desc.setObjectName('muted')
        layout.addWidget(desc)

        search_box = QHBoxLayout()
        self.search_input = QLineEdit()
        self.search_input.setPlaceholderText('Search Online by artist, album, track title, or paste URL / album ID…')
        self.search_input.returnPressed.connect(self._do_search)
        self.search_btn = QPushButton('Search online source')
        self.search_btn.clicked.connect(self._do_search)
        search_box.addWidget(self.search_input, 1)
        search_box.addWidget(self.search_btn)
        layout.addLayout(search_box)

        self.table = table(['Discovered Album', 'Discovered Artist', 'Year', 'Online ID', 'Matching Evidence'])
        self.table.setColumnWidth(0, 180)
        self.table.setColumnWidth(1, 150)
        self.table.setColumnWidth(2, 65)
        self.table.setColumnWidth(3, 90)
        self.table.horizontalHeader().setStretchLastSection(True)
        layout.addWidget(self.table, 1)

        display_rows = []
        for d in self.discoveries:
            display_rows.append((
                d.get('title', 'Unknown album'),
                d.get('discovered_artist', 'Unknown artist'),
                str(d.get('year', '')),
                str(d.get('release_id', '')),
                d.get('evidence', '')
            ))
        fill(self.table, display_rows)

        detail_box = QFrame()
        detail_box.setObjectName('overview_card')
        detail_box.setStyleSheet('''
            QFrame#overview_card {
                background-color: rgba(128, 128, 128, 0.08);
                border: 1px solid rgba(128, 128, 128, 0.18);
                border-radius: 8px;
                padding: 6px;
            }
            QFrame#overview_card:hover, QFrame#overview_card:focus {
                background-color: rgba(0, 122, 255, 0.10);
                border: 1px solid rgba(0, 122, 255, 0.65);
            }
        ''')
        db_layout = QVBoxLayout(detail_box)
        db_layout.setContentsMargins(10, 8, 10, 8)
        db_layout.setSpacing(4)
        self.detail_label = QLabel()
        self.detail_label.setWordWrap(True)
        db_layout.addWidget(self.detail_label)
        layout.addWidget(detail_box)

        btn_row = QHBoxLayout()
        btn_tidal = QPushButton('View on online source ↗')
        btn_tidal.clicked.connect(self._open_tidal)
        btn_row.addWidget(btn_tidal)
        btn_row.addStretch()

        self.btn_apply = QPushButton('Retag && Move to Artist')
        self.btn_apply.setObjectName('primary')
        self.btn_apply.clicked.connect(self._apply_chosen)
        btn_row.addWidget(self.btn_apply)

        btn_cancel = QPushButton('Cancel')
        btn_cancel.clicked.connect(self.reject)
        btn_row.addWidget(btn_cancel)
        layout.addLayout(btn_row)

        self.table.itemSelectionChanged.connect(self._update_detail)
        if self.discoveries:
            self.table.selectRow(0)
        self._update_detail()

    def _show_discoveries(self, found):
        existing={str(d['release_id']):d for d in self.discoveries}
        for d in found:existing[str(d['release_id'])]=d
        self.discoveries=list(existing.values())
        fill(self.table, [(d.get('title',''),d.get('discovered_artist',''),d.get('year',''),d['release_id'],d.get('evidence','')) for d in self.discoveries],keys=[str(d['release_id']) for d in self.discoveries])
        if found:self.table.selectRow(next(i for i in range(self.table.rowCount()) if self.table.item(i,3).text()==str(found[0]['release_id'])))
        self._update_detail()
        if not found:self.detail_label.setText('No verified recordings found. Try a release URL or a more specific album title.')
        self.search_btn.setEnabled(True);self.search_btn.setText('Search online source')

    def _do_search(self):
        query=self.search_input.text().strip();main=self.parent()
        if not query or not main:return
        if getattr(main,'demo_mode',False):
            self._show_discoveries([dict(release_id='300001',title=query.title()+' (Online Release)',year='2023',discovered_artist=query.title(),evidence='Fictional search result',tracks_matched=[dict(local_track=t,online_track_id='300002') for t in self.local_tracks])]);return
        if main.worker:return
        self.search_btn.setEnabled(False);self.search_btn.setText('Searching…')
        self.btn_apply.setEnabled(False)
        tracks=list(self.local_tracks)
        def work(cancel,progress):
            from .extended_review import discover
            return discover(tracks,main.store,main.market,main.api(cancel,progress),cancel,progress,query=query)
        def failed(message):
            self.detail_label.setText(message);self.search_btn.setEnabled(True);self.search_btn.setText('Search online source')
        main.job(work,self._show_discoveries,label='Search release matches',on_failure=failed)

    def _selected_discovery(self):
        item=self.table.item(self.table.currentRow(),3)
        return next((d for d in self.discoveries if str(d.get('release_id'))==item.text()),None) if item else None

    def _update_detail(self):
        d = self._selected_discovery()
        if not d:
            self.detail_label.setText('Select a discovered release above to review.')
            self.btn_apply.setEnabled(False)
            return
        self.btn_apply.setEnabled(True)
        artist = d.get('discovered_artist', '')
        self.btn_apply.setText('Preview tag changes and move…')
        matched_items = d.get('tracks_matched', [])
        if matched_items:
            matched_str = ', '.join(
                m.get('local_track', {}).get('title') or Path(m.get('local_track', {}).get('path', '')).stem
                for m in matched_items
            )
        else:
            matched_str = ', '.join(Path(p).stem for p in d.get('matched_files', [])) or 'All tracks for this release'
        self.detail_label.setText(
            f"<b>Release album artist:</b> {artist}<br>"
            f"<b>Matched Local File(s):</b> {matched_str}<br>"
            f"<b>Action:</b> Updates audio tags (albumartist = {artist}), moves files into {artist}/…, cleans up empty source folders, and clears stale artist review."
        )

    def _open_tidal(self):
        d = self._selected_discovery()
        if d and d.get('release_id'):
            QDesktopServices.openUrl(QUrl(f"https://tidal.com/album/{d['release_id']}"))

    def _apply_chosen(self):
        d = self._selected_discovery()
        if d:
            self.chosen_discovery = d
            self.accept()


class OnlineAlbumLinkDialog(QDialog):
    """Dialog comparing local release tags against candidate online releases to choose verified link."""
    def __init__(self, parent, row, options):
        super().__init__(parent)
        self.chosen_option = None
        self.row = row
        self.options = list(options or [])
        self.setWindowTitle('Online Album Link')
        self.resize(780, 560)

        layout = QVBoxLayout(self)
        layout.setSpacing(12)
        layout.setContentsMargins(18, 16, 18, 16)

        header = QVBoxLayout()
        header.setSpacing(4)
        title = QLabel('Online Album Link')
        title.setObjectName('title')
        font = title.font(); font.setPointSize(15); font.setBold(True); title.setFont(font)
        header.addWidget(title)
        subtitle = QLabel('Compare local file tags against candidate online releases and choose the verified link.')
        subtitle.setObjectName('muted')
        header.addWidget(subtitle)
        layout.addLayout(header)

        # Local File Box
        from .maintenance import first
        tags = row.get('tags', {})
        p = Path(row.get('path', ''))
        loc_group = QGroupBox('Local File Information')
        loc_box = QVBoxLayout(loc_group)
        loc_box.setSpacing(4)

        f_name = p.name
        f_artist = first(tags, 'albumartist') or first(tags, 'artist') or 'Unknown Artist'
        f_title = first(tags, 'title') or 'Unknown Title'
        f_album = first(tags, 'album') or 'Unknown Album'
        f_year = first(tags, 'date') or '—'
        f_bpm = first(tags, 'bpm') or '—'
        f_key = first(tags, 'initialkey') or first(tags, 'key') or '—'

        loc_box.addWidget(QLabel(f"<b>File:</b> {f_name}"))
        loc_info_line = QLabel(f"<b>Artist:</b> {f_artist} &nbsp;·&nbsp; <b>Album:</b> {f_album} ({f_year}) &nbsp;·&nbsp; <b>Title:</b> {f_title}")
        loc_box.addWidget(loc_info_line)
        loc_dj_line = QLabel(f"<b>DJ Tags:</b> BPM {f_bpm} &nbsp;·&nbsp; Key {f_key}")
        loc_dj_line.setObjectName('muted')
        loc_box.addWidget(loc_dj_line)
        from .release_matching import position,total
        disc,track=position(row)
        disc_total=total(row,'disc');track_total=total(row,'track')
        format_total=lambda n:f'{n:02d}' if n else '?'
        loc_box.addWidget(QLabel(f'Local position: Disc {disc:02d}/{format_total(disc_total)} · Track {track:02d}/{format_total(track_total)}'))
        if (disc_total and disc>disc_total) or (track_total and track>track_total):
            warning=QLabel('Invalid local total: the position exceeds the saved total. Review track/disc numbers in Correct tags; verified release details can establish the correct total.')
            warning.setWordWrap(True);loc_box.addWidget(warning)
        layout.addWidget(loc_group)

        # Candidate table
        layout.addWidget(QLabel('Linking saves a database association. File tags and folders stay unchanged.'))
        cand_label = QLabel(f'Verified Online Placements ({len(self.options)} available):')
        cand_label_font = cand_label.font(); cand_label_font.setBold(True); cand_label.setFont(cand_label_font)
        layout.addWidget(cand_label)

        self.table = table(['Artist', 'Album / Release', 'Year', 'Type', 'ID', 'Verified', 'Tag Changes', 'Remote position', 'Evidence'])
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        for c, w in [(0, 160), (1, 200), (2, 60), (3, 75), (4, 85), (5, 75)]:
            self.table.setColumnWidth(c, w)
        self.table.horizontalHeader().setStretchLastSection(True)
        layout.addWidget(self.table, 1)

        # Populate table using stable option identities.
        self.table.setSortingEnabled(False)
        for r_idx, opt in enumerate(self.options):
            if not opt.get('position_label') and hasattr(parent,'store'):
                cached=parent.store.preferences(f"tag-review:{parent.market}:{opt.get('id')}")
                remote_track=next((t for t in cached.get('tracks',[]) if str(t.get('id'))==str(opt.get('track_id'))),{})
                if remote_track:
                    rd=int(remote_track.get('disc_number') or 1);rt=int(remote_track.get('track_number') or 0)
                    per_disc=sum(int(t.get('disc_number') or 1)==rd for t in cached.get('tracks',[]))
                    opt['position_label']=f"Disc {rd:02d}/{cached.get('disc_count') or '?'} · Track {rt:02d}/{per_disc} · {len(cached.get('tracks',[]))} total · offsets disc {rd-disc:+d}, track {rt-track:+d}"
            ch_count = len(opt.get('changes', {}))
            ch_desc = f"{ch_count} change(s)" if ch_count else "Tags agree"
            ver_desc = 'Verified' if opt.get('recording_verified') else 'Candidate'
            row_items = [
                QTableWidgetItem(opt.get('artist') or '—'),
                QTableWidgetItem(opt.get('album') or '—'),
                QTableWidgetItem(str(opt.get('year') or opt.get('date') or '—')),
                QTableWidgetItem(format_release_type(opt.get('type') or 'ALBUM')),
                QTableWidgetItem(str(opt.get('id') or '—')),
                QTableWidgetItem(ver_desc),
                QTableWidgetItem(ch_desc),
                QTableWidgetItem(opt.get('position_label','Not captured · recheck candidate')),
                QTableWidgetItem('; '.join(opt.get('structure',{}).get('conflicts',[])) or opt.get('evidence','')),
            ]
            self.table.insertRow(r_idx)
            for c_idx, item in enumerate(row_items):
                item.setData(Qt.ItemDataRole.UserRole,r_idx)
                item.setFlags(Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable)
                self.table.setItem(r_idx, c_idx, item)

        self.table.setSortingEnabled(True)
        self.table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.table.customContextMenuRequested.connect(self.candidate_menu)
        # Detail preview for selected candidate
        self.detail_label = QLabel('Select an option above to preview proposed tag changes and DJ analysis.')
        self.detail_label.setObjectName('muted')
        self.detail_label.setWordWrap(True)
        layout.addWidget(self.detail_label)

        # Buttons
        btn_box = QHBoxLayout()
        btn_box.addStretch()
        cancel_btn = QPushButton('Cancel')
        cancel_btn.clicked.connect(self.reject)
        btn_box.addWidget(cancel_btn)
        self.link_btn = QPushButton('Link release')
        self.link_btn.setObjectName('primary')
        self.link_btn.clicked.connect(self._on_link_clicked)
        btn_box.addWidget(self.link_btn)
        layout.addLayout(btn_box)

        def on_selection_changed():
            sel = self.table.selectionModel().selectedRows()
            if not sel:
                self.link_btn.setEnabled(False)
                self.detail_label.setText('Select an option above to preview proposed tag changes and DJ analysis.')
                return
            idx = self.table.item(sel[0].row(),0).data(Qt.ItemDataRole.UserRole)
            if 0 <= idx < len(self.options):
                self.link_btn.setEnabled(True)
                opt = self.options[idx]
                details = []
                dj = opt.get('dj') or next((d for d in row.get('dj_checks',[]) if str(d.get('track_id'))==str(opt.get('track_id')) and str(d.get('album_id'))==str(opt.get('id'))),None)
                if dj:
                    bpm = dj.get('bpm')
                    key = dj.get('key')
                    bpm_str = ', '.join(bpm) if isinstance(bpm, list) else (str(bpm) if bpm else '—')
                    key_str = ', '.join(key) if isinstance(key, list) else (str(key) if key else '—')
                    details.append(f"🎧 <b>DJ Analysis:</b> BPM {bpm_str} · Key {key_str}")
                changes = opt.get('changes', {})
                if changes:
                    diffs = [f"{k}: “{v.get('old', '—')}” → “{v.get('new', '—')}”" if isinstance(v, dict) else f"{k}: {v}" for k, v in changes.items()]
                    details.append(f"✏️ <b>Tag Updates ({len(changes)}):</b> {', '.join(diffs[:4])}")
                else:
                    details.append("✓ Audio tags match this online release.")
                if opt.get('canonical_artist_conflict'):
                    details.append('Album Artist differs between equivalent editions. Existing credit retained; choose a tag correction explicitly.')
                    details.extend(f"Edition {e['album_id']}: {', '.join(e['album_artists'])}" for e in opt.get('artist_credit_evidence',[]))
                if opt.get('structure',{}).get('conflicts'):details.append('Structural conflicts: '+'; '.join(opt['structure']['conflicts']))
                if opt.get('position_label'):details.append(opt['position_label'])
                if opt.get('evidence'):
                    details.append(f"ℹ️ <b>Evidence:</b> {opt['evidence']}")
                self.detail_label.setText('<br>'.join(details))

        self.table.itemSelectionChanged.connect(on_selection_changed)
        if self.options:
            self.table.selectRow(0)

    def candidate_menu(self,pos):
        index=self.table.indexAt(pos)
        if not index.isValid():return
        self.table.selectRow(index.row())
        source=self.table.item(index.row(),0).data(Qt.ItemDataRole.UserRole)
        if not isinstance(source,int) or not 0<=source<len(self.options):return
        option=self.options[source];menu=QMenu(self)
        for label,kind,ident in [('Open release online','album',option.get('id')),('Open track online','track',option.get('track_id'))]:
            if ident and str(ident).isdigit():
                url=QUrl(f'https://tidal.com/{kind}/{ident}')
                menu.addAction(label).triggered.connect(lambda checked=False,url=url:QDesktopServices.openUrl(url))
        if menu.actions():menu.exec(self.table.viewport().mapToGlobal(pos))

    def _on_link_clicked(self):
        sel = self.table.selectionModel().selectedRows()
        if sel and 0 <= sel[0].row() < len(self.options):
            self.chosen_option = self.options[self.table.item(sel[0].row(),0).data(Qt.ItemDataRole.UserRole)]
            self.accept()
        else:
            self.reject()


class NavItem(QTreeWidgetItem):
    def __init__(self, strings=None, is_child=False):
        super().__init__()
        self._is_child = is_child
        self._raw_text = ""
        if strings:
            self._raw_text = strings[0]
            prefix = ""
            super().setText(0, prefix + strings[0])
            if is_child:
                font = self.font(0)
                font.setPointSizeF(11.5)
                self.setFont(0, font)

    def addChild(self, child):
        super().addChild(child)
        if isinstance(child, NavItem):
            child._is_child = True
            raw = child._raw_text or child.text(0)
            child._raw_text = raw.strip()
            super(NavItem, child).setText(0, child._raw_text)
            font = child.font(0)
            font.setPointSizeF(11.5)
            child.setFont(0, font)

    def setText(self, *args):
        if len(args) == 1:
            self._raw_text = args[0]
            prefix = ""
            super().setText(0, prefix + args[0])
        elif len(args) >= 2:
            if args[0] == 0:
                self._raw_text = args[1]
                prefix = ""
                super().setText(0, prefix + args[1])
            else:
                super().setText(*args)

    def text(self, col=0):
        if col == 0 and hasattr(self, '_raw_text') and self._raw_text:
            return self._raw_text
        return super().text(col)


class NavList(QTreeWidget):
    """Hierarchical navigation tree supporting expandable/collapsible submenus
    while maintaining 100% backwards compatibility with legacy QListWidget tests."""
    currentRowChanged = Signal(int)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setHeaderHidden(True)
        self.setRootIsDecorated(True)
        self.setIndentation(18)
        self.setAnimated(False)
        self.setExpandsOnDoubleClick(False)
        self._top_items = []
        self._current_row = 0
        self.currentItemChanged.connect(lambda item, previous: self._on_item_clicked(item, 0) if item else None)

    def count(self):
        return 7

    def currentRow(self):
        return self._current_row

    def item(self, i):
        if 0 <= i < len(self._top_items):
            return self._top_items[i]
        return None

    def setCurrentRow(self, row):
        self._current_row = row
        target_item = None
        if row in getattr(self,'_extra_items',{}):
            target_item=self._extra_items[row]
        elif row == 9:
            target_item = self._item_download.child(1)
        elif row == 6:
            target_item = getattr(self, '_item_settings', None) or (self._top_items[6] if len(self._top_items) > 6 else None)
        elif row == 7:
            target_item = getattr(self, '_item_favs', None) or (self._top_items[7] if len(self._top_items) > 7 else None)
        elif row == 8:
            target_item = getattr(self, '_item_activity', None) or (self._top_items[8] if len(self._top_items) > 8 else None)
        elif 0 <= row < len(self._top_items):
            target_item = self._top_items[row]

        if target_item:
            parent=target_item.parent()
            while parent:
                parent.setExpanded(True);parent=parent.parent()
            self.blockSignals(True)
            self.setCurrentItem(target_item)
            self.blockSignals(False)
            if target_item.childCount() > 0 and not target_item.isExpanded():
                target_item.setExpanded(True)
        self.currentRowChanged.emit(row)

    def _on_item_clicked(self, item, col):
        data = item.data(0, Qt.ItemDataRole.UserRole)
        if data is not None:
            page_index, action_cb = data
            self._current_row = page_index
            self.currentRowChanged.emit(page_index)
            if action_cb:
                action_cb()

    def addItems(self, items):
        pass


class Window(QMainWindow):
    authorize_url = Signal(str)
    review_updated = Signal(str)
    download_auth_requested = Signal(str, object)
    def __init__(self, store, demo_mode=False, credentials=None):
        super().__init__()
        self.store, self.demo_mode = store, demo_mode
        self.worker = None
        self._link_worker=None;self._link_enabled=False;self._link_restart=False;self._bg_probe_worker=None
        self.is_linking_active=False
        self.linking_completed={}
        self._last_background_link_check=0.0
        self._deferred_job=None
        self._closing_requested=False
        self._preview_worker=None;self._preview_pending=None;self._preview_error=False;self._view_worker=None;self._view_pending=False;self._view_data=None;self._coverage_worker=None;self._coverage_pending=None
        self.artist_rows=[];self.root_rows=[];self.coverage_rows=[];self._tool_counts={};self._summary_cache={}
        self._dirty_roots = set();self._prepared_roots=set();self._scan_generations={}
        self.expanded_releases=set();self._base_coverage_rows=[]
        self.expanded_queue_releases = set()
        self.cached_tidal_favourites = None
        from .organisation import DEFAULT_LAYOUT,LEGACY_LAYOUT
        layout=self.store.preferences('organisation')
        if layout.get('template')==LEGACY_LAYOUT:self.store.save_preferences('organisation',dict(layout,template=DEFAULT_LAYOUT))
        self.credentials = credentials or Credentials()
        from .client_settings import normalized
        self.provider_settings=normalized(self.store.preferences('provider'))
        self.account = Account(self.credentials,settings=lambda:self.provider_settings)
        self.request_pacer = RequestPacer(self.provider_settings['request_interval_ms']/1000)
        self.authorize_url.connect(self.open_authorization)
        self.download_auth_requested.connect(self.download_auth_dialog)
        self.market = 'DEMO' if demo_mode else os.getenv('TIDAL_MARKET', 'GB').upper()
        QApplication.setApplicationName("Tibrary")
        QApplication.setApplicationDisplayName("Tibrary")
        self.setWindowTitle('Tibrary' + (' — Fictional demo' if demo_mode else ''))
        self.resize(1240, 820)
        self.settings = QSettings('LocalLibrary', 'TidalManagerDemo' if demo_mode else 'TidalManager')
        self.download_folder = QLineEdit(str(self.settings.value('download_folder', str(Path.home() / 'Downloads/Music'))))
        self.download_folder.setReadOnly(True)
        self.download_folder.setMinimumHeight(34)
        QApplication.instance().styleHints().colorSchemeChanged.connect(self.apply_theme)

        root = QWidget()
        root_layout = QHBoxLayout(root)
        root_layout.setContentsMargins(14, 14, 14, 14)
        root_layout.setSpacing(14)

        self.nav = NavList()
        self.nav.setObjectName('nav_vertical')
        self.nav.setFixedWidth(245)
        self.nav.setItemDelegate(SidebarDelegate(self.nav))
        root_layout.addWidget(self.nav)

        self.activity_nav_btn = QPushButton('Activity')
        self.activity_nav_btn.hide()

        self.stack = QStackedWidget()
        root_layout.addWidget(self.stack, 1)
        self.setCentralWidget(root)
        self.startup_overlay = StartupOverlay(self)
        self.closing_overlay = ClosingOverlay(self)
        self.session_linked_count = 0
        self.session_total_to_check = 0
        self.session_link_start_time = None

        self.activity_log = QPlainTextEdit(); self.activity_log.setReadOnly(True)
        self.activity_log.setMaximumBlockCount(3000)
        self.activity_log.setPlaceholderText('Detailed steps, results, and errors appear here while you work.')

        self.overview_page()
        self.artists_page()
        self.link_releases_page()
        self.missing_page()
        self.queue_page()
        self.tools_page()
        self.settings_tabs = QTabWidget(); self.settings_tabs.setDocumentMode(False)
        self.settings_tabs.tabBar().setExpanding(False)
        self.settings_tabs.tabBar().setElideMode(Qt.TextElideMode.ElideNone)
        self.settings_tabs.tabBar().hide()
        self.settings_tabs.addTab(self.match_settings_page(), 'General')
        self.settings_tabs.addTab(self.connection_page(), 'Connections')
        self.settings_tabs.addTab(self.downloads_settings_page(), 'Downloads')
        self.stack.addWidget(self.settings_tabs)
        self.favourites_page_widget = self.favourites_page()
        self.stack.addWidget(self.favourites_page_widget)
        self.activity_page_widget = self.activity_page()
        self.stack.addWidget(self.activity_page_widget)
        self.downloaded_releases_page_widget = self.downloaded_releases_page()
        self.stack.addWidget(self.downloaded_releases_page_widget)

        from .audit_pages import AuditPage
        self.mqa_page=AuditPage(self, "mqa"); self.stack.addWidget(self.mqa_page)
        self.optimizations_page=AuditPage(self, "optimizations"); self.stack.addWidget(self.optimizations_page)
        self.online_optimizations_page=AuditPage(self, 'online_optimizations');self.stack.addWidget(self.online_optimizations_page)

        self.remediation_page()
        self.category_pages()
        self._init_nav_items()

        self._setup_menu_bar()
        self.nav.currentRowChanged.connect(self._on_nav_page_changed)
        self.nav.setCurrentRow(0)
        self.cancel = QPushButton('Cancel current job'); self.cancel.setEnabled(False)
        self.cancel.clicked.connect(self.cancel_job)
        controls = QWidget()
        controls_layout = QHBoxLayout(controls)
        controls_layout.setContentsMargins(12, 6, 12, 6)
        controls_layout.setSpacing(10)
        controls_layout.addWidget(self.cancel)
        self.background_link_indicator=QLabel('Linking releases in background');self.background_link_indicator.hide()
        controls_layout.addWidget(self.background_link_indicator)

        self.online_status_label = QLabel('☁️ Online API: Ready')
        self.online_status_label.setObjectName('muted')
        self.online_status_label.setContentsMargins(14, 0, 0, 0)
        self.online_status_label.hide()
        self.statusBar().addWidget(self.online_status_label)
        self.scan_activity=QLabel('Preparing library…');self.scan_activity.hide()
        self.statusBar().addWidget(self.scan_activity,1)
        self.statusBar().addPermanentWidget(controls)
        self.statusBar().setSizeGripEnabled(False)
        self.statusBar().hide()
        self._resume_bg_linking_after_job = False
        self._bg_linking_timer = QTimer(self)
        self._bg_linking_timer.setInterval(30000)
        self._bg_linking_timer.timeout.connect(self._check_background_linking)
        self._bg_linking_timer.start()
        self.activity_panel = QDockWidget('Activity details · this session', self)
        self.activity_panel.setAllowedAreas(Qt.DockWidgetArea.BottomDockWidgetArea)
        self.activity_panel.setFeatures(QDockWidget.DockWidgetFeature.DockWidgetClosable)
        self.activity_panel.setWidget(QWidget())
        self.addDockWidget(Qt.DockWidgetArea.BottomDockWidgetArea, self.activity_panel)
        self.resizeDocks([self.activity_panel], [190], Qt.Orientation.Vertical)
        self.activity_panel.hide()
        self.details_button = QPushButton('Show activity'); self.details_button.setCheckable(True)
        self.details_button.toggled.connect(self.activity_panel.setVisible)
        self.activity_panel.visibilityChanged.connect(self.details_button.setChecked)
        self.activity_panel.visibilityChanged.connect(lambda visible: self.details_button.setText('Hide activity' if visible else 'Show activity'))
        controls_layout.addWidget(self.details_button)
        self.elapsed_label = QLabel()
        controls_layout.addWidget(self.elapsed_label)
        self.busy = QProgressBar(); self.busy.setRange(0, 0); self.busy.setFixedWidth(75); self.busy.hide()
        controls_layout.addWidget(self.busy)
        self.job_timer = QTimer(self); self.job_timer.setInterval(1000)
        self.job_timer.timeout.connect(self.update_elapsed)
        self.review_updated.connect(self.update_artist_row)
        self.log('Ready. Choose a library, review artists, or open Settings → Connections.')
        self.change_theme(self.appearance.currentText())
        self.change_table_density(self.settings.value('table_density', 'Default (12 pt)'))
        self.refresh()
        if self.demo_mode:
            self.refresh_favourites_cache()
        elif getattr(self, 'credentials', None):
            try:
                if (getattr(self.credentials, 'session', None) or (hasattr(self, 'account') and self.account.logged_in())) and self.isVisible():
                    QTimer.singleShot(1500, lambda: self.isVisible() and Path(self.store.path).exists() and self.refresh_favourites_cache())
            except Exception:
                pass
        QTimer.singleShot(0, self._start_initialization)

    def download_session_connected(self):
        try:
            candidates = [
                self.store.path.parent / 'downloader-session' / 'tidaler' / 'token.json',
                Path.home() / '.config' / 'tidaler' / 'token.json',
            ]
            for p in candidates:
                if p.is_file():
                    data = json.loads(p.read_text('utf-8'))
                    if data.get('access_token') or data.get('refresh_token') or data.get('user_id'):
                        return True
        except Exception:
            pass
        return False

    def disconnect_download_session(self):
        try:
            candidates = [
                self.store.path.parent / 'downloader-session' / 'tidaler' / 'token.json',
                Path.home() / '.config' / 'tidaler' / 'token.json',
            ]
            for p in candidates:
                if p.is_file():
                    p.unlink(missing_ok=True)
            return 'Download subscriber session removed.'
        except Exception as exc:
            return f'Could not remove download session: {exc}'

    def _init_nav_items(self):
        item_overview = NavItem(['🎵  Overview'])
        item_overview.setData(0, Qt.ItemDataRole.UserRole, (0, None))
        self.nav.addTopLevelItem(item_overview)

        prepare=NavItem(['🧹  Prepare Library']);prepare.setData(0,Qt.ItemDataRole.UserRole,(5,lambda:self.tools_tabs.setCurrentIndex(0)));self.nav.addTopLevelItem(prepare)
        item_tools = prepare
        item_tools.setData(0, Qt.ItemDataRole.UserRole, (5, lambda:self.tools_tabs.setCurrentIndex(0)))
        correct=NavItem(['🏷️  Correct Tags']);correct.setData(0,Qt.ItemDataRole.UserRole,(5,lambda:self.tools_tabs.setCurrentIndex(1)));prepare.addChild(correct)
        organise=NavItem(['📁  Organise Files']);organise.setData(0,Qt.ItemDataRole.UserRole,(5,lambda:self.tools_tabs.setCurrentIndex(4)));prepare.addChild(organise)
        mqa=NavItem(['🔬  MQA Audit']);mqa.setData(0,Qt.ItemDataRole.UserRole,(10,None));prepare.addChild(mqa)

        link_group=NavItem(['🔗  Link Catalogue']);link_group.setData(0,Qt.ItemDataRole.UserRole,(14,None));self.nav.addTopLevelItem(link_group)
        item_artists = NavItem(['👤  Link Artists'])
        item_artists.setData(0, Qt.ItemDataRole.UserRole, (1, None))
        link_group.addChild(item_artists)

        item_releases = NavItem(['🔗  Link Releases'])
        item_releases.setData(0, Qt.ItemDataRole.UserRole, (2, None))
        link_group.addChild(item_releases)

        item_favs = NavItem(['⭐  Favourite Artists'])
        item_favs.setData(0, Qt.ItemDataRole.UserRole, (7, None))
        link_group.addChild(item_favs)

        fix=NavItem(['🛠️  Fix Library']);fix.setData(0,Qt.ItemDataRole.UserRole,(13,None));self.nav.addTopLevelItem(fix)
        complete=NavItem(['⬇️  Complete Library']);complete.setData(0,Qt.ItemDataRole.UserRole,(15,None));self.nav.addTopLevelItem(complete)

        item_missing = NavItem(['🔍  Missing Releases'])
        item_missing.setData(0, Qt.ItemDataRole.UserRole, (3, None))
        complete.addChild(item_missing)

        item_download = NavItem(['⬇️  Download Releases'])
        item_download.setData(0, Qt.ItemDataRole.UserRole, (4, None))
        complete.addChild(item_download)
        item_dl_queue = NavItem(['📥  Queue'])
        item_dl_queue.setData(0, Qt.ItemDataRole.UserRole, (4, None))
        item_download.addChild(item_dl_queue)
        item_dl_downloaded = NavItem(['✅  Downloaded Releases'])
        item_dl_downloaded.setData(0, Qt.ItemDataRole.UserRole, (9, self.refresh_downloaded_releases))
        item_download.addChild(item_dl_downloaded)

        add_tags=NavItem(['➕  Add Missing Tags']);add_tags.setData(0,Qt.ItemDataRole.UserRole,(5,lambda:self.tools_tabs.setCurrentIndex(2)));fix.addChild(add_tags)
        artwork=NavItem(['🖼️  Fix Artwork']);artwork.setData(0,Qt.ItemDataRole.UserRole,(5,lambda:self.tools_tabs.setCurrentIndex(3)));fix.addChild(artwork)
        optimizations=NavItem(['♻️  Local Consolidation']);optimizations.setData(0,Qt.ItemDataRole.UserRole,(11,None));prepare.addChild(optimizations)
        online_optimizations=NavItem(['♻️  Online Replacements']);online_optimizations.setData(0,Qt.ItemDataRole.UserRole,(12,None));fix.addChild(online_optimizations)
        self.nav_tool_children=[item_tools,correct,add_tags,artwork,organise]

        item_settings = NavItem(['⚙️  Settings'])
        item_settings.setData(0, Qt.ItemDataRole.UserRole, (16, None))
        self.nav.addTopLevelItem(item_settings)
        self.nav_settings_children = []
        for s_idx, s_name in enumerate(['⚙️  General', '🔑  Connections', '⬇️  Downloads']):
            c = NavItem([s_name])
            c.setData(0, Qt.ItemDataRole.UserRole, (6, lambda idx=s_idx: self.settings_tabs.setCurrentIndex(idx)))
            item_settings.addChild(c)
            self.nav_settings_children.append(c)

        item_activity = NavItem(['📋  Activity'])
        item_activity.setData(0, Qt.ItemDataRole.UserRole, (8, None))
        item_settings.addChild(item_activity)

        def on_tools_tab_changed(idx):
            if hasattr(self, 'nav_tool_children') and 0 <= idx < len(self.nav_tool_children):
                self.nav.setCurrentItem(self.nav_tool_children[idx])
        self.tools_tabs.currentChanged.connect(on_tools_tab_changed)

        def on_settings_tab_changed(idx):
            if hasattr(self, 'nav_settings_children') and 0 <= idx < len(self.nav_settings_children):
                self.nav.setCurrentItem(self.nav_settings_children[idx])
        self.settings_tabs.currentChanged.connect(on_settings_tab_changed)

        self.nav._top_items = [
            item_overview, item_artists, item_releases, item_missing,
            item_download, item_tools, item_settings, item_favs, item_activity
        ]
        self.nav._extra_items={14:link_group,15:complete,16:item_settings,10:mqa,11:optimizations,12:online_optimizations,13:fix}
        self.nav._item_settings = item_settings
        self.nav._item_favs = item_favs
        self.nav._item_activity = item_activity
        self.nav._item_download = item_download
        self.nav._item_tools = item_tools
        item_download.setExpanded(True)
        prepare.setExpanded(True)
        link_group.setExpanded(True)
        complete.setExpanded(True)
        item_settings.setExpanded(True)

    def resizeEvent(self, event):
        super().resizeEvent(event)
        if hasattr(self, '_reflow_diag_cards'):
            self._reflow_diag_cards(event.size().width())
        if hasattr(self, 'startup_overlay') and self.startup_overlay.isVisible():
            self.startup_overlay.resizeToParent()
        if hasattr(self, 'closing_overlay') and self.closing_overlay.isVisible():
            self.closing_overlay.resizeToParent()

    def _setup_menu_bar(self):
        menu_bar = self.menuBar()

        # File menu
        file_menu = menu_bar.addMenu('&File')
        self.menu_stop_job_act = file_menu.addAction('Stop current job')
        self.menu_stop_job_act.setShortcut(QKeySequence('Ctrl+.'))
        self.menu_stop_job_act.triggered.connect(self.cancel_current_job)
        self.menu_stop_job_act.setEnabled(False)

        self.menu_pause_link_act = file_menu.addAction('Pause background linking')
        self.menu_pause_link_act.setShortcut(QKeySequence('Ctrl+Shift+P'))
        self.menu_pause_link_act.triggered.connect(self.toggle_link_pause)
        self.menu_pause_link_act.setEnabled(False)
        file_menu.addSeparator()

        close_win_act = file_menu.addAction('Close window')
        close_win_act.setShortcut(QKeySequence('Ctrl+W'))
        close_win_act.triggered.connect(self.close)
        file_menu.addSeparator()
        quit_act = file_menu.addAction('Quit')
        quit_act.setMenuRole(QAction.MenuRole.QuitRole)
        quit_act.setShortcut(QKeySequence.StandardKey.Quit)
        quit_act.triggered.connect(self.close)

        file_menu.aboutToShow.connect(self._update_file_menu_actions)

        # View menu
        view_menu = menu_bar.addMenu('&View')
        screens = [
            ('Overview', 'Ctrl+1', lambda: self.nav.setCurrentRow(0)),
            ('Link Artists', 'Ctrl+2', lambda: self.nav.setCurrentRow(1)),
            ('Link Releases', 'Ctrl+3', lambda: self.nav.setCurrentRow(2)),
            ('Missing Releases', 'Ctrl+4', lambda: self.nav.setCurrentRow(3)),
            ('Download Releases', 'Ctrl+5', lambda: self.nav.setCurrentRow(4)),
            ('Library Tools', 'Ctrl+6', lambda: self.nav.setCurrentRow(5)),
            ('Favourite Artists', 'Ctrl+7', lambda: self.nav.setCurrentRow(7)),
            ('Settings', 'Ctrl+,', lambda: self.nav.setCurrentRow(6)),
        ]
        for name, shortcut, callback in screens:
            act = view_menu.addAction(name)
            act.setShortcut(shortcut)
            act.triggered.connect(callback)
        view_menu.addSeparator()
        act_act = view_menu.addAction('Activity')
        act_act.setShortcut('Ctrl+8')
        act_act.triggered.connect(self.show_activity_page)

        # Library menu
        lib_menu = menu_bar.addMenu('&Library')
        add_lib_act = lib_menu.addAction('Add library…')
        add_lib_act.setShortcut('Ctrl+O')
        add_lib_act.triggered.connect(self.add_library)
        update_lib_act = lib_menu.addAction('Update library')
        update_lib_act.setShortcut('Ctrl+U')
        update_lib_act.triggered.connect(self.rescan)
        refresh_tags_act = lib_menu.addAction('Refresh local tags')
        refresh_tags_act.setShortcut('Ctrl+R')
        refresh_tags_act.triggered.connect(self.inspect_library)
        refresh_online_tags_act = lib_menu.addAction('Refresh online tags')
        refresh_online_tags_act.setShortcut('Ctrl+Shift+R')
        refresh_online_tags_act.triggered.connect(lambda: self.start_linking(recheck=True))

        # Account / Service menu
        account_menu = menu_bar.addMenu('&Account')
        connect_sub_act = account_menu.addAction('Connect account…')
        connect_sub_act.triggered.connect(lambda: self.start_downloads(connect_only=True))
        test_conn_act = account_menu.addAction('Test connections')
        test_conn_act.triggered.connect(self.test_connections)
        account_menu.addSeparator()

        quality_menu = account_menu.addMenu('Audio Quality')
        self._quality_group = QActionGroup(self)
        self._quality_group.setExclusive(True)

        qual_options = [
            ('Lossless (FLAC)', 0),
            ('Hi-Res Lossless', 1),
            ('High (320 kbps)', 2),
            ('Low (96 kbps)', 3),
        ]
        for label, idx in qual_options:
            act = QAction(label, self)
            act.setCheckable(True)
            self._quality_group.addAction(act)
            quality_menu.addAction(act)
            def make_handler(index):
                return lambda: self._on_menu_quality_selected(index)
            act.triggered.connect(make_handler(idx))

        account_menu.addSeparator()
        view_favs_act = account_menu.addAction('Compare favourites…')
        view_favs_act.triggered.connect(self.show_favourites_dialog)
        refresh_favs_act = account_menu.addAction('Refresh favourites')
        refresh_favs_act.triggered.connect(self.refresh_favourites_cache)

        # Window menu
        win_menu = menu_bar.addMenu('&Window')
        min_act = win_menu.addAction('Minimize')
        min_act.setShortcut('Ctrl+M')
        min_act.triggered.connect(self.showMinimized)
        zoom_act = win_menu.addAction('Zoom')
        zoom_act.triggered.connect(lambda: self.showNormal() if self.isMaximized() else self.showMaximized())
        win_menu.addSeparator()
        front_act = win_menu.addAction('Bring all to front')
        front_act.triggered.connect(self.raise_)

        self._sync_menu_quality()
        if hasattr(self, 'download_quality'):
            self.download_quality.currentIndexChanged.connect(self._sync_menu_quality)

    def _sync_menu_quality(self, *args):
        if not hasattr(self, '_quality_group') or not hasattr(self, 'download_quality'):
            return
        idx = self.download_quality.currentIndex()
        actions = self._quality_group.actions()
        if 0 <= idx < len(actions):
            actions[idx].setChecked(True)

    def _on_menu_quality_selected(self, idx):
        if not hasattr(self, 'download_quality'):
            return
        self.download_quality.setCurrentIndex(idx)
        quality_val = self.download_quality.currentData() or self.download_quality.currentText()
        download = self.store.preferences('downloads') or {}
        replaygain = self.download_replaygain.isChecked() if hasattr(self, 'download_replaygain') else download.get('replaygain', True)
        segments = self.download_segments.value() if hasattr(self, 'download_segments') else download.get('segments', 1)
        cover_size = self.download_cover_size.currentData() if hasattr(self, 'download_cover_size') else download.get('cover_size', '1280')
        self.store.save_preferences('downloads', dict(download, quality=quality_val, cover_size=cover_size, replaygain=replaygain, segments=segments))
        self.log(f'Audio quality profile changed to: {self.download_quality.itemText(idx)}')

    def _load_cached_snapshot(self, root):
        import json
        rows = self.store.rows('SELECT path, root, size, mtime, metadata, error FROM local_files WHERE root=? AND present=1', (root,))
        from .freshness import cached_inspection
        inspected = {r['path']:r for r in cached_inspection(self.store, root)}
        snapshot = []
        layout = self.store.preferences('organisation')
        for r in rows:
            saved = inspected.get(r['path'])
            if saved and tuple(saved.get('stamp',()))[2:4] == (r['size'],r['mtime']):
                snapshot.append(dict(saved, layout=layout));continue
            tags = {}
            duration = 0
            if r.get('metadata'):
                try:
                    raw_meta = json.loads(r['metadata'])
                    if isinstance(raw_meta, dict):
                        duration = float(raw_meta.get('duration') or 0)
                        for k, v in raw_meta.items():
                            if v is not None and v != '':
                                tags[k] = list(v) if isinstance(v, (list, tuple)) else [str(v)]
                        if 'bpm' in raw_meta and raw_meta['bpm']:
                            tags['bpm'] = [str(raw_meta['bpm'])]
                        if 'musical_key' in raw_meta and raw_meta['musical_key']:
                            tags['initialkey'] = [str(raw_meta['musical_key'])]
                except Exception:
                    tags = {}
            if 'artist' in tags and 'albumartist' not in tags:
                tags['albumartist'] = list(tags['artist'])
            if 'track' in tags:tags['tracknumber']=tags.pop('track')
            if 'track_artist' in tags:tags['artist']=tags.pop('track_artist')
            snapshot.append(dict(
                path=r['path'],
                root=r['root'],
                target=r['path'],
                changes={},
                issues=[],
                blocked=r.get('error') or '',
                organise=False,
                layout=layout,
                stamp=(r['size'], r['mtime']),
                tags=tags,
                has_artwork=False,
                duration=duration,
                cover_size=None
            ))
        from .linking import attach_links
        attach_links(snapshot, self.store, self.market)
        return snapshot

    def _start_initialization(self):
        try:
            roots = self.store.rows('SELECT * FROM roots ORDER BY root')
        except Exception:
            if hasattr(self, 'startup_overlay'):
                self.startup_overlay.hide()
            return
        if not roots or self.demo_mode:
            if hasattr(self, 'startup_overlay'):
                self.startup_overlay.hide()
            return

        primary_root = roots[0]['root']
        size = self.store.rows('SELECT COUNT(*) AS n FROM local_files WHERE present=1')[0]['n']

        # Pre-populate Library Tools and tables immediately from local database on launch
        cached_init = self._load_cached_snapshot(primary_root)
        if cached_init:
            self.tools_snapshots[primary_root] = cached_init
            self.tools_plan = cached_init
            self._update_all_tool_counts(cached_init)
            self.render_tools_plan()
        for page in (self.mqa_page,self.optimizations_page,self.online_optimizations_page):
            page.load_library(primary_root)

        def do_init(cancel=lambda: False, progress=lambda s: None):
            from .freshness import prepare_library
            if not Path(primary_root).is_dir():
                snapshot = self._load_cached_snapshot(primary_root)
                return {'root': primary_root, 'snapshot': snapshot, 'offline': True}
            snapshot = prepare_library(self.store, primary_root, cached_init, cancel, progress)
            return {'root': primary_root, 'snapshot': snapshot, 'offline': False}

        def on_complete(result):
            if hasattr(self, 'startup_overlay'):
                self.startup_overlay.hide()
            if not isinstance(result, dict) or 'snapshot' not in result:
                return
            root = result['root']
            snapshot = result['snapshot']
            self.tools_snapshots[root] = snapshot
            self.tools_plan = snapshot
            self._dirty_roots.discard(root)
            self.mqa_page.result_root=None;self.mqa_page.load_library(root)
            self._update_all_tool_counts(snapshot)
            self.render_tools_plan()
            if result.get('offline'):
                self.log(f'Drive for {Path(root).name} is offline. Loaded cached library tags from database.')
            else:
                self.log(f'Library tags verified for {Path(root).name} ({len(snapshot):,} files).')
            self.refresh()
            QTimer.singleShot(500, self._check_background_linking)

        if size < 200:
            try:
                res = do_init()
                on_complete(res)
            except Exception:
                if hasattr(self, 'startup_overlay'):
                    self.startup_overlay.hide()
            return

        if hasattr(self, 'startup_overlay'):
            self.startup_overlay.show()
            self.startup_overlay.resizeToParent()
            self.startup_overlay.raise_()
            self.startup_overlay.status_label.setText(f'Checking local tags · {Path(primary_root).name}…')

        self._startup_worker = Worker(do_init)
        self._startup_worker.message.connect(
            lambda msg: hasattr(self, 'startup_overlay') and self.startup_overlay.status_label.setText(msg),
            Qt.ConnectionType.QueuedConnection
        )
        self._startup_worker.result.connect(on_complete, Qt.ConnectionType.QueuedConnection)
        self._startup_worker.failure.connect(
            lambda err: (hasattr(self, 'startup_overlay') and self.startup_overlay.hide(), self.log(f'Initialisation: {err}')),
            Qt.ConnectionType.QueuedConnection
        )
        self._startup_worker.start()

    def change_theme(self, mode):
        if mode not in ('System', 'Light', 'Dark'): mode='System'
        self.settings.setValue('appearance',mode)
        scheme={'System':Qt.ColorScheme.Unknown,'Light':Qt.ColorScheme.Light,'Dark':Qt.ColorScheme.Dark}[mode]
        QApplication.instance().styleHints().setColorScheme(scheme)
        self.apply_theme()

    def change_table_density(self, mode):
        self.settings.setValue('table_density', mode)
        pt = 11 if '11' in mode else (13 if '13' in mode else 12)
        row_h = 26 if pt == 11 else (32 if pt == 13 else 28)
        font = self.font()
        font.setPointSize(pt)
        for tbl in (getattr(self, 'queue_table', None),
                    getattr(self, 'coverage_table', None),
                    getattr(self, 'artist_table', None),
                    getattr(self, 'tools_table', None),
                    getattr(self, 'local_table', None),
                    getattr(self, 'roots', None),
                    getattr(self, 'activity', None)):
            if tbl:
                tbl.setFont(font)
                tbl.verticalHeader().setDefaultSectionSize(row_h)

    def apply_theme(self, *args):
        app = QApplication.instance()
        mode = self.appearance.currentText() if hasattr(self, 'appearance') else 'System'
        dark = mode == 'Dark' or (mode == 'System' and app.palette().color(QPalette.ColorRole.Window).lightness() < 128)

        chevron_url = CHEVRON_ICON_PATH.replace('\\', '/')
        if dark:
            tab_bg = '#1C1C1E'
            tab_border = 'rgba(255, 255, 255, 0.12)'
            tab_hover_bg = 'rgba(255, 255, 255, 0.08)'
            tab_color = 'rgba(255, 255, 255, 0.90)'
        else:
            tab_bg = '#E3E3E8'
            tab_border = 'rgba(0, 0, 0, 0.10)'
            tab_hover_bg = 'rgba(255, 255, 255, 0.5)'
            tab_color = 'rgba(0, 0, 0, 0.85)'

        stylesheet = STYLE.replace('__CHEVRON_ICON_PATH__', chevron_url) \
                          .replace('__TAB_BG__', tab_bg) \
                          .replace('__TAB_BORDER__', tab_border) \
                          .replace('__TAB_COLOR__', tab_color) \
                          .replace('__TAB_HOVER_BG__', tab_hover_bg)
        if dark:
            stylesheet += '\nQHeaderView::section { border: 0px; border-right: 1px solid black; border-bottom: 1px solid black; }'
        app.setStyleSheet(stylesheet)

    @Slot(str)
    def log(self, message):
        safe = self.credentials.redact(message).replace('\r', ' ').replace('\n', ' ')
        self.last_step = time.monotonic()
        lower = safe.lower()
        # Page/request chatter adds little beyond the surrounding operation summary.
        if safe.startswith(('Catalogue request ·', 'Fetching ', 'Authentication ·', 'Authentication succeeded', 'Using cached release summaries ·')) \
                or 'page ' in lower or ('finished ' in lower and 'across ' in lower) \
                or 'cached release does not fit the current tags' in lower:
            return
        import re
        safe = re.sub(r'\bTIDAL\b', 'Online', safe)
        safe = re.sub(r'\btidal\b', 'online', safe)
        safe = re.sub(r'\bTidal\b', 'Online', safe)
        line = f'{datetime.now():%H:%M:%S}  {safe}'
        if not hasattr(self, '_raw_activity_lines'):
            self._raw_activity_lines = []
        self._raw_activity_lines.append(line)
        if len(self._raw_activity_lines) > 3000:
            self._raw_activity_lines = self._raw_activity_lines[-3000:]
        query = getattr(self, 'activity_filter_input', None)
        filter_text = query.text().strip().lower() if query else ''
        if not filter_text or filter_text in line.lower():
            self.activity_log.appendPlainText(line)
            if getattr(self, 'activity_autoscroll', None) and self.activity_autoscroll.isChecked():
                from PySide6.QtGui import QTextCursor
                self.activity_log.moveCursor(QTextCursor.MoveOperation.End)
        self.last_step = time.monotonic()


    def update_elapsed(self):
        elapsed = int(time.monotonic() - self.job_started)
        waiting = int(time.monotonic() - self.last_step)
        time_str = f'{elapsed // 60}:{elapsed % 60:02d} elapsed' + (f' · {waiting}s since update' if waiting >= 5 else '')
        self.elapsed_label.setText(time_str)
        if hasattr(self, 'activity_job_detail') and self.worker:
            self.activity_job_detail.setText(f'{time_str} · {getattr(self, "job_label", "Working…")}')

    def cancel_job(self):
        if self.worker:
            self.worker.requestInterruption()
            self.log('Cancellation requested · waiting for the current file or network request to return (up to 20 seconds for online catalogue).')

    def cancel_current_job(self):
        if getattr(self, 'worker', None) and self.worker.isRunning():
            self.cancel_job()
        elif getattr(self, '_link_worker', None) and self._link_worker.isRunning():
            self.pause_linking()
        elif getattr(self, '_preview_worker', None) and self._preview_worker.isRunning():
            self._preview_worker.requestInterruption()
            self.log('Stopping current preview job…')
        elif getattr(self, '_view_worker', None) and self._view_worker.isRunning():
            self._view_worker.requestInterruption()
            self.log('Stopping current library view build…')

    def _update_file_menu_actions(self):
        has_active_worker = any(bool(getattr(self, a, None) and getattr(self, a).isRunning())
                                for a in ('worker', '_preview_worker', '_view_worker'))
        link_running = bool(getattr(self, '_link_worker', None) and self._link_worker.isRunning()
                            and not self._link_worker.isInterruptionRequested())
        if hasattr(self, 'menu_stop_job_act'):
            self.menu_stop_job_act.setEnabled(bool(has_active_worker or link_running))

        if hasattr(self, 'menu_pause_link_act'):
            if link_running:
                self.menu_pause_link_act.setText('Pause background linking')
                self.menu_pause_link_act.setEnabled(True)
            elif getattr(self, '_link_enabled', False) or getattr(self, 'session_link_start_time', None):
                self.menu_pause_link_act.setText('Resume background linking')
                self.menu_pause_link_act.setEnabled(True)
            else:
                self.menu_pause_link_act.setText('Background linking')
                self.menu_pause_link_act.setEnabled(False)

    def open_authorization(self, url):
        if not QDesktopServices.openUrl(QUrl(url)):
            self.log('Could not open your browser for account sign-in. Set a default browser and retry.')
            if self.worker: self.worker.requestInterruption()

    def load_favourites(self, cancel, progress, refresh=False):
        try:
            profile = self.account.load()
        except CredentialError:
            if refresh: raise
            progress('Account session unavailable · connect your Online account to enable favourites-first lookup')
            return []
        if not profile:
            if refresh: raise CatalogueError('Connect your Online account before refreshing favourites.')
            progress('No user account connected · client credentials provide catalogue search only; favourites need account sign-in')
            return []
        cached = self.store.favourites(profile['cache_id'])
        if cached and not refresh:
            artists = json.loads(cached['payload'])
            progress(f'Checking {len(artists)} cached favourited artists · snapshot {format_user_datetime(cached["fetched"])}')
            return artists
        api = Tidal(self.market if self.market != 'DEMO' else 'GB', cancel, progress=progress, user_token=self.account.token, pacer=self.request_pacer,settings=self.provider_settings)
        artists = api.favourite_artists()
        if cancel(): raise CatalogueError('Favourites refresh cancelled; previous snapshot retained.')
        self.store.save_favourites(profile['cache_id'], artists)
        progress(f'Saved {len(artists)} available favourited artists · all collection pages checked')
        return artists

    def connection_dialog(self):
        self.nav.setCurrentRow(6)
        self.settings_tabs.setCurrentIndex(1)

    @staticmethod
    def settings_section(parent,title):
        escaped_title = title.replace('&', '&&') if '&&' not in title else title
        group = QGroupBox(escaped_title)
        group.setSizePolicy(QSizePolicy.Policy.Preferred, QSizePolicy.Policy.Maximum)
        body = QVBoxLayout(group)
        body.setContentsMargins(12,10,12,10);body.setSpacing(8)
        parent.addWidget(group)
        return body

    @staticmethod
    def settings_details(parent,title):
        escaped_title = title.replace('&', '&&') if '&&' not in title else title
        button = QPushButton(f'▸ {escaped_title}')
        button.setCheckable(True)
        button.setStyleSheet("""
            QPushButton {
                text-align: left;
                background-color: rgba(128, 128, 128, 0.08);
                border: 1px solid rgba(128, 128, 128, 0.15);
                border-radius: 7px;
                padding: 7px 14px;
                font-size: 13px;
                font-weight: 500;
                color: palette(window-text);
            }
            QPushButton:hover {
                background-color: rgba(128, 128, 128, 0.15);
            }
            QPushButton:checked {
                background-color: rgba(0, 122, 255, 0.12);
                border-color: rgba(0, 122, 255, 0.35);
                color: #007AFF;
                font-weight: 600;
            }
        """)
        parent.addWidget(button)
        panel = QFrame()
        panel.setObjectName('settingsReferencePanel')
        panel.setStyleSheet("""
            QFrame#settingsReferencePanel {
                background-color: rgba(128, 128, 128, 0.04);
                border: 1px solid rgba(128, 128, 128, 0.12);
                border-radius: 8px;
                margin-top: 4px;
            }
        """)
        body = QVBoxLayout(panel)
        body.setContentsMargins(14, 12, 14, 12)
        body.setSpacing(10)
        parent.addWidget(panel)
        panel.hide()
        def toggle(opened):
            panel.setVisible(opened)
            symbol = '▾' if opened else '▸'
            button.setText(f'{symbol} {escaped_title}')
        button.toggled.connect(toggle)
        return body

    def connection_page(self):
        page = QWidget()
        root_layout = QVBoxLayout(page)
        root_layout.setContentsMargins(0, 4, 0, 14)
        root_layout.setSpacing(24)
        heading = QLabel('Connections')
        heading.setObjectName('title')
        root_layout.addWidget(heading)
        sub = QLabel('Catalogue access, favourites and extended metadata.')
        sub.setWordWrap(True)
        sub.setObjectName('muted')
        root_layout.addWidget(sub)
        dialog = page

        # Section 1: Online account
        acc_layout = self.settings_section(root_layout, 'Online account')
        acc_info = QLabel('Sign in with your Online account to compare favourited artists, retrieve lossless audio streams, and enrich local metadata.')
        acc_info.setWordWrap(True)
        acc_layout.addWidget(acc_info)

        account_row = QHBoxLayout()
        connect_account = QPushButton('Connect account in browser')
        download_connect = QPushButton('Connect subscriber account')
        disconnect_account = QPushButton('Disconnect account')
        account_row.addWidget(connect_account)
        account_row.addWidget(download_connect)
        account_row.addWidget(disconnect_account)
        account_row.addStretch()
        acc_layout.addLayout(account_row)

        setup_box = QFrame()
        setup_box.setObjectName('connectionSetupPanel')
        setup_box.setStyleSheet("""
            QFrame#connectionSetupPanel {
                background-color: rgba(128, 128, 128, 0.05);
                border: 1px solid rgba(128, 128, 128, 0.15);
                border-radius: 8px;
            }
        """)
        setup_layout = QVBoxLayout(setup_box)
        setup_layout.setContentsMargins(14, 12, 14, 12)
        setup_layout.setSpacing(8)
        account_note = QLabel('In your developer portal, enable collection.read and search.read scopes, and register this OAuth redirect URI:')
        account_note.setWordWrap(True); setup_layout.addWidget(account_note)
        red_row = QHBoxLayout()
        redirect = QLineEdit(REDIRECT); redirect.setReadOnly(True); redirect.setMinimumHeight(34)
        copy_uri = QPushButton('📋 Copy URI')
        def copy_redirect():
            QApplication.clipboard().setText(REDIRECT)
            copy_uri.setText('✓ Copied!')
            QTimer.singleShot(2000, lambda: copy_uri.setText('📋 Copy URI'))
        copy_uri.clicked.connect(copy_redirect)
        red_row.addWidget(redirect, 1); red_row.addWidget(copy_uri)
        setup_layout.addLayout(red_row)
        acc_layout.addWidget(setup_box)

        # Section 2: Developer API credentials
        dev_layout = self.settings_section(root_layout, 'Developer API credentials')
        dev_intro = QLabel('Client ID and client secret from your Online developer app. Credentials are saved securely in macOS Keychain.')
        dev_intro.setWordWrap(True); dev_layout.addWidget(dev_intro)
        form = QFormLayout(); client = QLineEdit(); client.setObjectName('client_id')
        secret = QLineEdit(); secret.setObjectName('client_secret')
        client.setEchoMode(QLineEdit.EchoMode.Password)
        secret.setEchoMode(QLineEdit.EchoMode.Password)
        has_keys = False
        try:
            has_keys = bool(self.credentials.has_keys() if hasattr(self.credentials, 'has_keys') else self.credentials.get())
        except Exception:
            has_keys = False
        if has_keys:
            client.setPlaceholderText('••••••••••••••••')
            secret.setPlaceholderText('••••••••••••••••')
            client.setReadOnly(True)
            secret.setReadOnly(True)
        else:
            client.setPlaceholderText('Enter client ID')
            secret.setPlaceholderText('Enter client secret')
            client.setReadOnly(False)
            secret.setReadOnly(False)
        form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)
        for field in (client, secret):
            field.setMinimumHeight(40); field.setMinimumWidth(300)
            field.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        form.addRow('Client ID', client); form.addRow('Client secret', secret); dev_layout.addLayout(form)

        cred_row = QHBoxLayout()
        save = QPushButton('Save credentials'); save.setObjectName('primary')
        remove = QPushButton('Forget saved credentials')
        editing_credentials={'active':False}
        if has_keys:save.setText('Credentials saved');save.setEnabled(False)
        cred_row.addWidget(save); cred_row.addWidget(remove); cred_row.addStretch()
        dev_layout.addLayout(cred_row)

        # Section 3: Diagnostics & Connection testing
        diag_layout = self.settings_section(root_layout, 'Connection diagnostics')

        def make_diag_card(title, initial_metric, initial_sub):
            card = QFrame()
            card.setObjectName('overview_card')
            card.setMinimumHeight(0)
            card.setSizePolicy(QSizePolicy.Policy.Expanding,QSizePolicy.Policy.Preferred)
            card.setStyleSheet('''
                QFrame#overview_card {
                    background-color: rgba(128, 128, 128, 0.08);
                    border: 1px solid rgba(128, 128, 128, 0.18);
                    border-radius: 10px;
                }
                QLabel#card_title {
                    font-size: 11px;
                    font-weight: 700;
                    text-transform: uppercase;
                    letter-spacing: 0.5px;
                }
                QLabel#card_metric { font-size: 19px; font-weight: 700; }
                QLabel#card_sub { font-size: 12px; }
            ''')
            card_l = QVBoxLayout(card)
            card_l.setContentsMargins(14, 12, 14, 12)
            card_l.setSpacing(4)
            card_l.setAlignment(Qt.AlignmentFlag.AlignTop)
            tl = QLabel(title); tl.setObjectName('card_title'); card_l.addWidget(tl)
            ml = QLabel(initial_metric); ml.setObjectName('card_metric'); card_l.addWidget(ml)
            sl = QLabel(initial_sub); sl.setObjectName('card_sub'); sl.setWordWrap(True); card_l.addWidget(sl)
            return card, ml, sl

        diag_cards = QGridLayout()
        diag_cards.setSpacing(10)
        card_api, self.diag_api_metric, self.diag_api_sub = make_diag_card(
            '🌐 Online API', 'Ready' if self.demo_mode else 'Active', 'https://openapi.tidal.com/v2'
        )
        has_browser = bool(hasattr(self, 'account') and hasattr(self.account, 'logged_in') and self.account.logged_in())
        card_browser, self.diag_browser_metric, self.diag_browser_sub = make_diag_card(
            '👤 Browser Account', 'Connected' if has_browser else 'Disconnected', 'OAuth collection & favourites'
        )
        has_download = bool(self.download_session_connected())
        card_download, self.diag_down_metric, self.diag_down_sub = make_diag_card(
            '⬇️ Download Session', 'Connected' if has_download else 'Disconnected', 'Subscriber audio stream token'
        )
        card_keys, self.diag_keys_metric, self.diag_keys_sub = make_diag_card(
            '🔑 Keychain', 'Configured' if has_keys else 'Missing', 'macOS Keychain (Tibrary)'
        )
        self.diag_user_metric = self.diag_browser_metric
        self._diag_cards_layout = diag_cards
        self._diag_card_widgets = [card_api, card_browser, card_download, card_keys]
        self._reflow_diag_cards(self.width())
        diag_layout.addLayout(diag_cards)

        test = QPushButton('Test configured connection')
        self.diag_checked=QLabel('Not tested this session');self.diag_checked.setObjectName('muted')
        test_row = QHBoxLayout(); test_row.addWidget(test);test_row.addWidget(self.diag_checked); test_row.addStretch()
        diag_layout.addLayout(test_row)
        class AutoStatusLabel(QLabel):
            def setText(self,text):
                super().setText(text);self.setVisible(bool(str(text).strip()))
        status = AutoStatusLabel();status.setObjectName('muted');status.setWordWrap(True);status.hide()
        diag_layout.addWidget(status)
        self.connection_status = status

        def saving():
            if client.isReadOnly():
                editing_credentials['active']=True
                client.setReadOnly(False);secret.setReadOnly(False)
                client.setPlaceholderText('Enter replacement client ID')
                secret.setPlaceholderText('Enter replacement client secret')
                save.setText('Save replacement credentials')
                client.setFocus()
                status.setText('Your current credentials remain saved until the replacement is stored successfully.')
                return
            if not client.text().strip() or not secret.text().strip():
                status.setText('Enter both the client ID and secret.'); return
            pair = (client.text(), secret.text())
            def work(cancel, progress):
                progress('Saving credentials · securely storing in macOS Keychain')
                return self.credentials.save(*pair, remember=True)
            def done(result):
                editing_credentials['active']=False
                client.clear(); secret.clear()
                client.setPlaceholderText('••••••••••••••••')
                secret.setPlaceholderText('••••••••••••••••')
                client.setReadOnly(True)
                secret.setReadOnly(True)
                save.setText('Credentials saved')
                self.diag_keys_metric.setText('Configured')
                status.setText(result)
            self.job(work, done, label='Configure online credentials', on_failure=status.setText)

        def testing():
            if client.text() or secret.text():
                status.setText('Save the entered credentials before testing.'); return
            self.test_connections()

        def forgetting():
            def work(cancel, progress):
                self.credentials.forget()
                return 'Saved credentials removed.'
            def done(result):
                editing_credentials['active']=False
                client.setPlaceholderText('Enter client ID')
                secret.setPlaceholderText('Enter client secret')
                client.setReadOnly(False)
                secret.setReadOnly(False)
                save.setText('Save credentials')
                self.diag_keys_metric.setText('Missing')
                status.setText(result)
            self.job(work, done, label='Remove saved credentials', on_failure=status.setText)

        save.clicked.connect(saving); test.clicked.connect(testing); remove.clicked.connect(forgetting)

        def connect_user():
            self.job(lambda cancel, progress: self.account.connect(self.authorize_url.emit, cancel, progress, True),
                     status.setText, label='Connect browser account', on_failure=status.setText)
        connect_account.clicked.connect(connect_user)

        def disconnect_all():
            def work(cancel, progress):
                res = []
                if hasattr(self, 'account') and hasattr(self.account, 'logged_in') and self.account.logged_in():
                    res.append(self.account.disconnect())
                if self.download_session_connected():
                    res.append(self.disconnect_download_session())
                return ' · '.join(res) if res else 'No active account sessions to disconnect.'
            self.job(work, status.setText, label='Disconnect accounts', on_failure=status.setText)
        disconnect_account.clicked.connect(disconnect_all)

        download_connect.clicked.connect(lambda: self.start_downloads(connect_only=True))

        timer = QTimer(dialog)
        def update_buttons():
            is_idle = self.worker is None
            has_b = bool(hasattr(self, 'account') and hasattr(self.account, 'logged_in') and self.account.logged_in())
            has_d = bool(self.download_session_connected())
            has_k = False
            try:
                has_k = bool(self.credentials.has_keys() if hasattr(self.credentials, 'has_keys') else self.credentials.get())
            except Exception:
                has_k = False
            editing=bool(editing_credentials['active'] and has_k)
            client.setReadOnly(has_k and not editing)
            secret.setReadOnly(has_k and not editing)
            client.setEnabled(is_idle);secret.setEnabled(is_idle)
            save.setText('Save replacement credentials' if editing else ('Credentials saved' if has_k else 'Save credentials'))
            save.setEnabled(is_idle and not has_k)
            remove.setEnabled(is_idle and has_k)
            test.setEnabled(is_idle)
            connect_account.setEnabled(is_idle and not has_b)
            if has_b:
                connect_account.setText('Browser account connected')
            else:
                connect_account.setText('Connect account in browser')

            download_connect.setEnabled(is_idle and not has_d)
            if has_d:
                download_connect.setText('Subscriber account connected')
            else:
                download_connect.setText('Connect subscriber account')

            disconnect_account.setEnabled(is_idle and (has_b or has_d))
            self.diag_browser_metric.setText('Connected' if has_b else 'Disconnected')
            self.diag_down_metric.setText('Connected' if has_d else 'Disconnected')
            self.diag_keys_metric.setText('Configured' if has_k else 'Missing')
        timer.timeout.connect(update_buttons); timer.start(200); update_buttons()
        root_layout.addStretch()
        for text_label in dialog.findChildren(QLabel): text_label.setWordWrap(True)
        dialog.setSizePolicy(QSizePolicy.Policy.Ignored, QSizePolicy.Policy.Preferred)
        self.connection_page_widget = dialog
        return self.scroll_page(dialog)

    def _reflow_diag_cards(self, width=None):
        if not hasattr(self, '_diag_cards_layout') or not hasattr(self, '_diag_card_widgets'):
            return
        if width is None:
            width = self.width()
        for w in self._diag_card_widgets:
            self._diag_cards_layout.removeWidget(w)
        if width >= 850:
            for idx, w in enumerate(self._diag_card_widgets):
                self._diag_cards_layout.addWidget(w, 0, idx)
        else:
            self._diag_cards_layout.addWidget(self._diag_card_widgets[0], 0, 0)
            self._diag_cards_layout.addWidget(self._diag_card_widgets[1], 0, 1)
            self._diag_cards_layout.addWidget(self._diag_card_widgets[2], 1, 0)
            self._diag_cards_layout.addWidget(self._diag_card_widgets[3], 1, 1)

    def match_settings_page(self):
        page = QWidget()
        root_layout = QVBoxLayout(page)
        root_layout.setContentsMargins(0, 4, 0, 14)
        root_layout.setSpacing(24)
        heading = QLabel('General')
        heading.setObjectName('title')
        root_layout.addWidget(heading)
        sub = QLabel('Library preferences and storage settings.')
        sub.setWordWrap(True)
        sub.setObjectName('muted')
        root_layout.addWidget(sub)
        lib_layout = self.settings_section(root_layout, 'Local libraries')
        self.roots = table(['Library root', 'Last successful scan', 'Status'])
        self.roots.setMaximumHeight(120)
        self.roots.itemSelectionChanged.connect(self._on_roots_selection_changed)
        lib_layout.addWidget(self.roots)

        lib_form = QFormLayout()
        lib_form.setFormAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop)
        lib_form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft)
        lib_form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)

        self.drive_mode_check = QCheckBox('Multi-drive mode'); self.drive_mode_check.hide()
        saved_mode = str(self.settings.value('drive_mode', 'single')).lower()
        self.drive_mode_check.setChecked('multi' in saved_mode)
        self.drive_mode_check.toggled.connect(lambda checked: self.settings.setValue('drive_mode', 'multi' if checked else 'single'))
        self.drive_mode_combo = QComboBox(); self.drive_mode_combo.hide()

        self.active_drive_combo = QComboBox()
        self.active_drive_combo.currentIndexChanged.connect(self._on_active_drive_changed)
        lib_form.addRow('Active library', self.active_drive_combo)
        lib_layout.addLayout(lib_form)

        lib_actions = QHBoxLayout()
        btn_add = QPushButton('Add library…'); btn_add.setObjectName('primary')
        btn_add.clicked.connect(self.add_library)
        btn_update = QPushButton('Update library')
        btn_update.clicked.connect(self.rescan)
        btn_recheck = QPushButton('Refresh local tags')
        btn_recheck.clicked.connect(lambda: self.rescan(force=True))
        lib_actions.addWidget(btn_add)
        lib_actions.addWidget(btn_update)
        lib_actions.addWidget(btn_recheck)
        lib_actions.addStretch()
        lib_layout.addLayout(lib_actions)

        layout=self.settings_section(root_layout,'Appearance')
        appearance_form=QFormLayout();self.appearance=QComboBox();self.appearance.addItems(['System','Light','Dark'])
        appearance_form.setFormAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop)
        appearance_form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft)
        appearance_form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)
        self.appearance.setCurrentText(self.settings.value('appearance','System'))
        appearance_form.addRow('Theme',self.appearance)
        self.table_density = QComboBox()
        self.table_density.addItems(['Compact (11 pt)', 'Default (12 pt)', 'Comfortable (13 pt)'])
        saved_density = self.settings.value('table_density', 'Default (12 pt)')
        idx = self.table_density.findText(saved_density)
        if idx >= 0: self.table_density.setCurrentIndex(idx)
        appearance_form.addRow('Table font size', self.table_density)
        self.table_density.currentTextChanged.connect(self.change_table_density)
        if not hasattr(self, 'activity_autoscroll'):
            self.activity_autoscroll = QCheckBox('Auto-scroll activity log')
            self.activity_autoscroll.setChecked(True)
        appearance_form.addRow('Activity log', self.activity_autoscroll)
        layout.addLayout(appearance_form)
        self.appearance.currentTextChanged.connect(self.change_theme)

        # Retain hidden artist matching controls for backwards compatibility
        preferences = self.store.match_preferences()
        self.auto_match = QCheckBox(page); self.auto_match.setChecked(preferences['enabled']); self.auto_match.hide()
        self.match_threshold = QSpinBox(page); self.match_threshold.setRange(50,98); self.match_threshold.setValue(preferences['threshold']); self.match_threshold.hide()
        self.match_margin = QSpinBox(page); self.match_margin.setRange(10,50); self.match_margin.setValue(preferences['margin']); self.match_margin.hide()
        def save_match():
            self.store.save_match_preferences(dict(enabled=self.auto_match.isChecked(), threshold=self.match_threshold.value(), margin=self.match_margin.value()))
            self.log('Matching settings saved · apply to the next batch; existing manual decisions remain unchanged')
        self.auto_match.toggled.connect(save_match)
        self.match_threshold.valueChanged.connect(save_match)
        self.match_margin.valueChanged.connect(save_match)
        self._hidden_match_btn = QPushButton('Save matching settings', page)
        self._hidden_match_btn.clicked.connect(save_match)
        self._hidden_match_btn.hide()

        layout=self.settings_section(root_layout,'Discovery cache')
        link_form=QFormLayout();link_form.setFormAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop);link_form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft);link_form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)
        self.link_cache_age=QComboBox()
        for label,days in [('7 days',7),('30 days',30),('90 days',90),('Only when I recheck',0)]:self.link_cache_age.addItem(label,days)
        saved_age=self.store.preferences('release_links',{'max_age_days':30}).get('max_age_days',30)
        self.link_cache_age.setCurrentIndex(self.link_cache_age.findData(saved_age if saved_age in (0,7,30,90) else 30))
        link_form.addRow('Cache duration',self.link_cache_age);layout.addLayout(link_form)
        note=QLabel('Reuse verified release links to prevent redundant Online lookups. Opening a page never starts online checks.')
        note.setWordWrap(True);layout.addWidget(note)
        self.link_cache_age.currentIndexChanged.connect(self.save_link_cache_settings)

        cat_layout = self.settings_section(root_layout, 'Catalogue & database')
        cat_note = QLabel('Import a previously exported or backed-up catalogue to restore or prime your offline artist cache.')
        cat_note.setWordWrap(True)
        cat_layout.addWidget(cat_note)
        self.buttons(cat_layout, [('Import catalogue…', self.import_catalogue, False)])
        root_layout.addStretch()
        return self.scroll_page(page)

    def save_link_cache_settings(self):
        self.store.save_preferences('release_links',{'max_age_days':self.link_cache_age.currentData()})
        self.refresh_coverage()

    def backend_versions(self):
        if self.worker: return
        from .backends import versions, check_upstream_updates
        def _check(cancel, progress):
            progress('Checking current streaming component versions…')
            vers = versions()
            progress('Checking for streaming component updates…')
            up = check_upstream_updates()
            return {'versions': vers, 'upstream': up}
        def _done(res):
            vers_text = res.get('versions', '')
            self.backend_status.setText('Streaming components checked successfully.')
            up = res.get('upstream', {})
            if up.get('updates_available'):
                msg = "A streaming component update is available.\n\nWould you like to build and verify it now? The current version remains active unless all checks pass."
                if QMessageBox.question(self, 'Streaming Update Available', msg,
                                        QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No) == QMessageBox.StandardButton.Yes:
                    self.update_backend(confirmed=True)
            else:
                QMessageBox.information(self, 'Components Up to Date','The streaming components are up to date.')
        self.job(_check, _done, label='Check component versions', on_failure=self.backend_status.setText)

    def update_backend(self, confirmed=False):
        if self.worker or self.demo_mode: return
        if not confirmed:
            if QMessageBox.question(self, 'Update streaming components',
                                    'Build the latest streaming and metadata components in an isolated runtime? The current installation stays active until compatibility checks pass.') != QMessageBox.StandardButton.Yes:
                return
        from .backends import update
        self.job(lambda cancel, progress: update(cancel, progress), self.backend_status.setText, label='Update streaming components', on_failure=self.backend_status.setText)

    def rollback_backend(self):
        if self.worker or self.demo_mode:return
        from .backends import rollback
        self.job(lambda cancel,progress:rollback(),self.backend_status.setText,label='Restore previous streaming components',on_failure=self.backend_status.setText)

    def template_reference(self,layout,editor):
        details=self.settings_details(layout,'Template reference & tag variables')
        table_tags = [
            ('{albumartist}', 'Album artist name'),
            ('{album}', 'Release / album title'),
            ('{year}', 'Release year (4 digits)'),
            ('{disc}', 'Subfolder "Disc 1", "Disc 2" (for multi-disc releases only)'),
            ('{disc_prefix}', 'Disc prefix "1.", "2." (for multi-disc filenames)'),
            ('{tracknumber}', 'Two-digit track number (e.g. 01, 02)'),
            ('{title}', 'Track title'),
            ('{genre}', 'Primary genre (if tagged)'),
            ('{label}', 'Record label (if tagged)'),
        ]
        grid = QGridLayout()
        grid.setSpacing(6)
        for row_i, (tag_name, tag_desc) in enumerate(table_tags):
            btn_tag = QPushButton(tag_name)
            btn_tag.setCursor(Qt.CursorShape.PointingHandCursor)
            btn_tag.setToolTip(f"Click to insert {tag_name} into template at cursor position")
            btn_tag.setFixedHeight(34)
            btn_tag.setSizePolicy(QSizePolicy.Policy.Preferred, QSizePolicy.Policy.Fixed)
            btn_tag.clicked.connect(lambda _, t=tag_name: editor.insert(t))
            lbl_desc = QLabel(tag_desc)
            lbl_desc.setObjectName('muted')
            lbl_desc.setFixedHeight(34)
            lbl_desc.setAlignment(Qt.AlignmentFlag.AlignVCenter | Qt.AlignmentFlag.AlignLeft)
            grid.addWidget(btn_tag, row_i, 0)
            grid.addWidget(lbl_desc, row_i, 1)
        details.addLayout(grid)

    def downloads_settings_page(self):
        page = QWidget()
        root_layout = QVBoxLayout(page)
        root_layout.setContentsMargins(0, 4, 0, 14)
        root_layout.setSpacing(24)
        heading = QLabel('Downloads')
        heading.setObjectName('title')
        root_layout.addWidget(heading)
        root_layout.addWidget(QLabel('Configure audio quality, download destination, and streaming limits.'))

        loc_layout = self.settings_section(root_layout, 'Download folder & structure')
        loc_layout.addWidget(QLabel('Folder where approved music is downloaded and organized:'))
        loc_row = QHBoxLayout(); loc_row.setContentsMargins(0,0,0,0); loc_row.setSpacing(8)
        loc_row.addWidget(self.download_folder, 1)
        choose_btn = QPushButton('Choose folder…')
        choose_btn.setMinimumHeight(34)
        choose_btn.clicked.connect(self.choose_download_folder)
        loc_row.addWidget(choose_btn)
        loc_layout.addLayout(loc_row)


        qual_layout = self.settings_section(root_layout, 'Audio quality')
        download = self.store.preferences('downloads')
        qual_form = QFormLayout(); qual_form.setContentsMargins(0,0,0,0); qual_form.setVerticalSpacing(12); qual_form.setHorizontalSpacing(16)
        self.download_quality = QComboBox()
        self.download_quality.addItem('FLAC lossless · 16-bit / 44.1 kHz', 'Lossless')
        self.download_quality.addItem('Hi-Res lossless · up to 24-bit / 192 kHz', 'Hi-res lossless')
        self.download_quality.addItem('High · 320 kbps AAC', 'High')
        self.download_quality.addItem('Low · 96 kbps AAC', 'Low')
        self.download_quality.text = lambda: self.download_quality.currentText()
        saved_q = str(download.get('quality', 'Lossless')).strip()
        idx = self.download_quality.findData(saved_q)
        if idx < 0:
            for i in range(self.download_quality.count()):
                if self.download_quality.itemData(i).lower() == saved_q.lower():
                    idx = i; break
        if idx >= 0:
            self.download_quality.setCurrentIndex(idx)
        else:
            self.download_quality.setCurrentIndex(0)

        self.download_cover_size = QComboBox()
        self.download_cover_size.addItem('1280 × 1280 (Default)', '1280')
        self.download_cover_size.addItem('Source / Original resolution', 'origin')
        self.download_cover_size.addItem('640 × 640', '640')
        self.download_cover_size.addItem('320 × 320', '320')
        saved_cs = str(download.get('cover_size', '1280')).strip().lower()
        cs_idx = self.download_cover_size.findData(saved_cs)
        if cs_idx >= 0:
            self.download_cover_size.setCurrentIndex(cs_idx)
        else:
            self.download_cover_size.setCurrentIndex(0)

        self.download_segments = QSpinBox()
        self.download_segments.setRange(1, 4)
        self.download_segments.setValue(download.get('segments', 2))
        self.download_segments.hide()
        qual_form.setFormAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop)
        qual_form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft)
        qual_form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)
        self.download_quality.setMinimumHeight(34); self.download_cover_size.setMinimumHeight(34)
        qual_form.addRow('Audio quality profile', self.download_quality)
        qual_form.addRow('Embedded artwork size', self.download_cover_size)
        self.download_bitrate_cap=QComboBox()
        self.download_bitrate_cap.addItem('320 kbps','320');self.download_bitrate_cap.addItem('96 kbps','96')
        self.download_bitrate_cap.setCurrentIndex(max(0,self.download_bitrate_cap.findData(str(self.provider_settings['aac_bitrate_cap']))))
        self.download_bitrate_cap.setMinimumHeight(34)
        qual_form.addRow('AAC bitrate cap',self.download_bitrate_cap)
        qual_layout.addLayout(qual_form)

        self.download_replaygain = QCheckBox('Write supplied ReplayGain volume tags')
        self.download_replaygain.setChecked(download.get('replaygain', True))
        self.download_replaygain.hide()

        def save_downloads():
            quality_val = self.download_quality.currentData() or self.download_quality.currentText()
            cover_size_val = self.download_cover_size.currentData() or '1280'
            cur = self.store.preferences('downloads')
            self.store.save_preferences('downloads', dict(cur, quality=quality_val, cover_size=cover_size_val, replaygain=self.download_replaygain.isChecked(), segments=self.download_segments.value()))
            self.log(f'Audio quality set to {self.download_quality.currentText()}. Artwork size set to {self.download_cover_size.currentText()}.')
        self.download_quality.currentIndexChanged.connect(save_downloads)
        self.download_cover_size.currentIndexChanged.connect(save_downloads)

        columns=QHBoxLayout();columns.setSpacing(16);root_layout.addLayout(columns)
        engine_column=QVBoxLayout()
        metadata_column=QVBoxLayout()
        columns.addLayout(engine_column,1);columns.addLayout(metadata_column,1)
        root_layout.removeWidget(qual_layout.parentWidget());engine_column.addWidget(qual_layout.parentWidget())
        qual_layout.parentWidget().setTitle('Download engine settings')
        engine_layout=qual_layout
        engine_form=qual_form;engine_form.setVerticalSpacing(12);engine_form.setHorizontalSpacing(16);engine_form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter)
        stream_layout=self.settings_section(metadata_column,'Additional metadata source settings')
        engine_layout.parentWidget().setSizePolicy(QSizePolicy.Policy.Preferred,QSizePolicy.Policy.Preferred)
        stream_layout.parentWidget().setSizePolicy(QSizePolicy.Policy.Preferred,QSizePolicy.Policy.Preferred)
        engine_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        stream_layout.setAlignment(Qt.AlignmentFlag.AlignTop)
        stream_form=QFormLayout();stream_form.setContentsMargins(0,0,0,0);stream_form.setVerticalSpacing(12);stream_form.setHorizontalSpacing(16)
        stream_form.setFormAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop)
        stream_form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignVCenter)
        stream_form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)
        def spin(value,low,high,suffix=''):
            widget=QSpinBox();widget.setRange(low,high);widget.setValue(int(value));widget.setSuffix(suffix);widget.setMinimumHeight(34);widget.setMinimumWidth(150);return widget
        self.provider_download_concurrency=spin(self.provider_settings['download_concurrency'],1,4)
        self.provider_segment_concurrency=spin(self.provider_settings['segment_concurrency'],1,20)
        self.provider_rate_interval=spin(self.provider_settings['request_interval_ms'],100,5000,' ms')
        self.provider_batch_size=spin(self.provider_settings['api_batch_size'],1,100)
        self.provider_timeout=spin(self.provider_settings['request_timeout_sec'],5,60,' s')
        self.provider_token_margin=spin(self.provider_settings['token_refresh_margin_sec'],10,300,' s')
        self.provider_sign_in_timeout=spin(self.provider_settings['sign_in_timeout_sec'],60,600,' s')
        self.provider_attempts=spin(self.provider_settings['request_attempts'],1,4)
        self.provider_download_delay=QCheckBox('Space download requests to reduce service throttling')
        self.provider_download_delay.setChecked(self.provider_settings['download_delay'])
        engine_form.addRow('Maximum parallel downloads',self.provider_download_concurrency)
        engine_form.addRow('Connections per audio file',self.provider_segment_concurrency)
        stream_form.addRow('Minimum API request spacing',self.provider_rate_interval)
        stream_form.addRow('Albums per request batch',self.provider_batch_size)
        stream_form.addRow('Network request timeout',self.provider_timeout)
        stream_form.addRow('Renew token before expiry',self.provider_token_margin)
        stream_form.addRow('Browser sign-in timeout',self.provider_sign_in_timeout)
        stream_form.addRow('Request attempts',self.provider_attempts)
        engine_form.addRow('',self.provider_download_delay)
        stream_layout.addLayout(stream_form)

        self.provider_delay_min=QDoubleSpinBox();self.provider_delay_max=QDoubleSpinBox();self.provider_batch_delay=QDoubleSpinBox()
        for widget,key in ((self.provider_delay_min,'download_delay_min_sec'),(self.provider_delay_max,'download_delay_max_sec'),(self.provider_batch_delay,'api_batch_delay_sec')):
            widget.setRange(0,60);widget.setDecimals(1);widget.setSingleStep(.5);widget.setValue(float(self.provider_settings[key]));widget.setSuffix(' s');widget.setMinimumHeight(34);widget.setMinimumWidth(150)
        engine_form.addRow('Minimum release pause',self.provider_delay_min)
        engine_form.addRow('Maximum release pause',self.provider_delay_max)
        stream_form.addRow('Pause between request batches',self.provider_batch_delay)

        folder_layout=loc_layout
        from .organisation import DEFAULT_LAYOUT
        self.streaming_layout_template=QLineEdit(self.store.preferences('organisation').get('template',DEFAULT_LAYOUT))
        self.streaming_layout_template.setMinimumHeight(34)
        folder_layout.addWidget(QLabel('Uses tag variables and the same Windows/macOS-safe path rules as Library Tools.'))
        folder_layout.addWidget(self.streaming_layout_template)
        self.template_reference(folder_layout,self.streaming_layout_template)
        self.streaming_layout_status=QLabel('')
        self.streaming_layout_status.setWordWrap(True);self.streaming_layout_status.hide()
        folder_layout.addWidget(self.streaming_layout_status)

        def save_provider_settings():
            from .client_settings import normalized
            values=normalized(dict(request_interval_ms=self.provider_rate_interval.value(),request_timeout_sec=self.provider_timeout.value(),
                token_refresh_margin_sec=self.provider_token_margin.value(),sign_in_timeout_sec=self.provider_sign_in_timeout.value(),request_attempts=self.provider_attempts.value(),
                download_concurrency=self.provider_download_concurrency.value(),segment_concurrency=self.provider_segment_concurrency.value(),
                download_delay=self.provider_download_delay.isChecked(),download_delay_min_sec=self.provider_delay_min.value(),
                download_delay_max_sec=self.provider_delay_max.value(),api_batch_size=self.provider_batch_size.value(),
                api_batch_delay_sec=self.provider_batch_delay.value(),aac_bitrate_cap=int(self.download_bitrate_cap.currentData())))
            self.provider_settings=values;self.store.save_preferences('provider',values)
            self.request_pacer.configure(values['request_interval_ms']/1000)

        def save_streaming_layout():
            self.streaming_layout_status.show()
            from .organisation import validate_layout
            try:validate_layout(self.streaming_layout_template.text())
            except ValueError as exc:
                self.streaming_layout_status.setText(f'Check the folder structure: {exc}')
                self.streaming_layout_status.setProperty('state','error')
                self.streaming_layout_status.style().unpolish(self.streaming_layout_status);self.streaming_layout_status.style().polish(self.streaming_layout_status)
                return
            organisation=self.store.preferences('organisation');organisation['template']=self.streaming_layout_template.text()
            self.store.save_preferences('organisation',organisation)
            self.invalidate_layout_preview()
            self.streaming_layout_status.setText('Folder structure saved.')
            self.streaming_layout_status.setProperty('state','success')
            self.streaming_layout_status.style().unpolish(self.streaming_layout_status);self.streaming_layout_status.style().polish(self.streaming_layout_status)
        for widget in (self.provider_download_concurrency,self.provider_segment_concurrency,self.provider_rate_interval,self.provider_batch_size,
                       self.provider_timeout,self.provider_token_margin,self.provider_sign_in_timeout,self.provider_attempts,self.provider_delay_min,self.provider_delay_max,self.provider_batch_delay):
            widget.valueChanged.connect(save_provider_settings)
        self.provider_download_delay.toggled.connect(save_provider_settings)
        self.download_bitrate_cap.currentIndexChanged.connect(save_provider_settings)
        self.streaming_layout_template.editingFinished.connect(save_streaming_layout)
        def update_delay_controls(enabled):
            self.provider_delay_min.setEnabled(enabled);self.provider_delay_max.setEnabled(enabled)
        self.provider_download_delay.toggled.connect(update_delay_controls)
        update_delay_controls(self.provider_download_delay.isChecked())
        self.provider_reset = QPushButton('Reset metadata settings to defaults')
        stream_layout.addStretch(1)
        stream_layout.addWidget(self.provider_reset, alignment=Qt.AlignmentFlag.AlignLeft)
        def reset_provider_settings(engine=False):
            from .client_settings import DEFAULTS
            widgets = {
                'request_interval_ms': self.provider_rate_interval, 'request_timeout_sec': self.provider_timeout,
                'token_refresh_margin_sec': self.provider_token_margin, 'sign_in_timeout_sec': self.provider_sign_in_timeout,
                'request_attempts': self.provider_attempts, 'download_concurrency': self.provider_download_concurrency,
                'segment_concurrency': self.provider_segment_concurrency, 'api_batch_size': self.provider_batch_size,
                'download_delay_min_sec': self.provider_delay_min, 'download_delay_max_sec': self.provider_delay_max,
                'api_batch_delay_sec': self.provider_batch_delay}
            engine_keys={'download_concurrency','segment_concurrency','download_delay_min_sec','download_delay_max_sec'}
            for key, widget in widgets.items():
                if (key in engine_keys)!=engine:continue
                previous=widget.blockSignals(True)
                widget.setValue(DEFAULTS[key]);widget.blockSignals(previous)
            if engine:
                previous=self.provider_download_delay.blockSignals(True)
                self.provider_download_delay.setChecked(DEFAULTS['download_delay']);self.provider_download_delay.blockSignals(previous)
                previous=self.download_bitrate_cap.blockSignals(True)
                self.download_bitrate_cap.setCurrentIndex(self.download_bitrate_cap.findData(DEFAULTS['aac_bitrate_cap']))
                self.download_bitrate_cap.blockSignals(previous)
                update_delay_controls(self.provider_download_delay.isChecked())
                self.download_quality.setCurrentIndex(self.download_quality.findData('Lossless'))
                self.download_cover_size.setCurrentIndex(self.download_cover_size.findData('1280'))
                self.download_replaygain.setChecked(True)
                save_downloads()
            save_provider_settings()
        self.provider_reset.clicked.connect(lambda:reset_provider_settings(False))
        self.download_engine_reset=QPushButton('Reset download engine to defaults')
        engine_layout.addStretch(1)
        engine_layout.addWidget(self.download_engine_reset,alignment=Qt.AlignmentFlag.AlignLeft)
        self.download_engine_reset.clicked.connect(lambda:reset_provider_settings(True))


        self._hidden_down_btn = QPushButton('Save download settings', page)
        self._hidden_down_btn.clicked.connect(save_downloads)
        self._hidden_down_btn.hide()

        layout = self.settings_section(root_layout, 'Streaming components & diagnostics')
        self.backend_status = QLabel()
        self.backend_status.setWordWrap(True)
        self.backend_status.setVisible(False)
        layout.addWidget(self.backend_status)
        btn_grid = QGridLayout()
        btn_grid.setSpacing(8)
        diag_btns = [
            ('Open diagnostic logs', lambda: QDesktopServices.openUrl(QUrl.fromLocalFile(str(self.store.path.parent / 'logs')))),
            ('Check component versions', self.backend_versions),
            ('Update streaming components…', self.update_backend),
            ('Restore previous components', self.rollback_backend)
        ]
        for i, (txt, fn) in enumerate(diag_btns):
            b = QPushButton(txt)
            b.clicked.connect(fn)
            btn_grid.addWidget(b, i // 2, i % 2)
        layout.addLayout(btn_grid)
        root_layout.addStretch()
        return self.scroll_page(page)

    tidaler_settings_page = downloads_settings_page

    @staticmethod
    def scan_summary(value):
        if not value: return ''
        try:
            data = json.loads(value)
            return f"{data.get('read', 0):,} tags read · {data.get('unchanged', 0):,} unchanged · {data.get('missing', 0):,} removed from index · {data.get('errors', 0):,} unreadable"
        except (ValueError, TypeError): return value

    def test_connections(self):
        if self.demo_mode or self.worker: return
        from .connections import check_connections
        def work(cancel, progress):
            result = check_connections(self.credentials, self.account, self.api(cancel, progress), progress,details=True)
            if cancel(): return result
            from .downloads import check_download_connection
            started=time.monotonic()
            download_status = check_download_connection(self.store)
            progress(download_status)
            result['download']=dict(ok='connected' in download_status.casefold() and 'not connected' not in download_status.casefold(),latency_ms=round((time.monotonic()-started)*1000),message=download_status)
            result['summary']+='\n'+download_status
            return result
        self.connection_status.setText('Checking connections…')
        def done(result):
            if not isinstance(result,dict):
                self.connection_status.setText(str(result));self._connection_metrics={'check':{'ok':False}}
                if self._view_data:self.update_category_cards(self._view_data)
                return
            metrics=result.get('metrics',{})
            def show(metric,label,sub,good='Connected',bad='Needs attention'):
                info=metrics.get(metric,{})
                label.setText(good if info.get('ok') else bad)
                latency=info.get('latency_ms')
                sub.setText((f'{latency:,} ms · ' if latency is not None else '')+info.get('message','Not checked'))
            show('search',self.diag_api_metric,self.diag_api_sub,'Ready','Error')
            show('account',self.diag_browser_metric,self.diag_browser_sub)
            show('credentials',self.diag_keys_metric,self.diag_keys_sub,'Configured','Missing')
            metrics['download']=result.get('download',{})
            show('download',self.diag_down_metric,self.diag_down_sub)
            ready=sum(bool(metrics.get(key,{}).get('ok')) for key in ('search','account','credentials','download'))
            self._connection_metrics=dict(metrics)
            if self._view_data:self.update_category_cards(self._view_data)
            self.connection_status.setText(f'Connection check complete · {ready} of 4 services ready. Detailed steps are available in Activity.')
            self.diag_checked.setText(f'Last checked {datetime.now():%H:%M:%S}')

        self.job(work, done, label='Check connections', on_failure=done)

    def api(self, cancel, progress):
        from .cached_tidal import CachedTidal
        return CachedTidal(self.market if self.market != 'DEMO' else os.getenv('TIDAL_MARKET', 'GB'), cancel,
                     credentials=self.credentials.get, progress=progress, search_user_token=self.account.search_token, pacer=self.request_pacer,settings=self.provider_settings,store=self.store)

    @staticmethod
    def scroll_page(widget):
        scroll = QScrollArea()
        scroll.setFrameShape(QFrame.Shape.NoFrame)
        scroll.setWidgetResizable(True)
        scroll.viewport().setBackgroundRole(QPalette.ColorRole.Window)
        scroll.setWidget(widget)
        return scroll

    def page(self, title, subtitle):
        page = QWidget(); layout = QVBoxLayout(page); layout.setContentsMargins(0, 4, 0, 0); layout.setSpacing(16)
        heading = QLabel(title); heading.setObjectName('title'); layout.addWidget(heading)
        sub = QLabel(subtitle); sub.setWordWrap(True); sub.setObjectName('muted'); layout.addWidget(sub)
        self.stack.addWidget(page)
        return layout

    def buttons(self, layout, actions):
        grid=len(actions)>4
        row=QGridLayout() if grid else QHBoxLayout()
        row.setSpacing(8)
        columns=3 if len(actions)<=6 else 4
        for index,(text,slot,primary) in enumerate(actions):
            button=QPushButton(text);button.setMinimumHeight(30)
            if primary:button.setObjectName('primary')
            button.clicked.connect(slot)
            if grid:row.addWidget(button,index//columns,index%columns)
            else:row.addWidget(button)
        if not grid:row.addStretch()
        layout.addLayout(row)

    def overview_page(self):
        layout = self.page('Overview', 'Your collection, track resolution status, and acquisition pipeline.')

        card_style = DASHBOARD_CARD_STYLE

        def make_stat_card(title, initial_metric, initial_sub, btn_text, action):
            card = ActionCard()
            card.setObjectName('overview_card')
            card.setStyleSheet(card_style)
            card.activated.connect(action)
            card_layout = QVBoxLayout(card)
            card_layout.setContentsMargins(14, 12, 14, 12)
            card_layout.setSpacing(4)

            title_lbl = QLabel(title)
            title_lbl.setObjectName('card_title')
            title_lbl.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents)
            card_layout.addWidget(title_lbl)

            metric_lbl = QLabel(initial_metric)
            metric_lbl.setObjectName('card_metric')
            metric_lbl.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents)
            card_layout.addWidget(metric_lbl)

            sub_lbl = QLabel(initial_sub)
            sub_lbl.setObjectName('card_sub')
            sub_lbl.setWordWrap(True)
            sub_lbl.setAttribute(Qt.WidgetAttribute.WA_TransparentForMouseEvents)
            card_layout.addWidget(sub_lbl)

            card_layout.addSpacing(2)

            btn_layout = QHBoxLayout()
            btn = QPushButton(btn_text)
            btn.setObjectName('card_btn')
            btn.clicked.connect(action)
            btn_layout.addWidget(btn)
            btn_layout.addStretch()
            return card, metric_lbl, sub_lbl, btn

        self.overview_session_banner = QFrame()
        self.overview_session_banner.setObjectName('overview_banner')
        self.overview_session_banner.setFixedHeight(42)
        self.overview_session_banner.setStyleSheet('''
            QFrame#overview_banner {
                background-color: rgba(0, 122, 255, 0.08);
                border: 1px solid rgba(0, 122, 255, 0.25);
                border-radius: 8px;
            }
        ''')
        ob_layout = QHBoxLayout(self.overview_session_banner)
        ob_layout.setContentsMargins(14, 0, 14, 0)
        ob_layout.setSpacing(12)
        ob_layout.setAlignment(Qt.AlignmentFlag.AlignVCenter)
        self.overview_session_banner_label = QLabel('🔄 Linking in background…')
        self.overview_session_banner_label.setStyleSheet('font-weight: 500; font-size: 12px;')
        self.overview_session_banner_label.setAlignment(Qt.AlignmentFlag.AlignVCenter | Qt.AlignmentFlag.AlignLeft)
        ob_layout.addWidget(self.overview_session_banner_label, 1, Qt.AlignmentFlag.AlignVCenter)
        ob_btn = QPushButton('View in Activity →')
        ob_btn.setObjectName('card_btn')
        ob_btn.setFixedHeight(26)
        ob_btn.setStyleSheet('QPushButton#card_btn { padding: 3px 10px; margin: 0; }')
        ob_btn.clicked.connect(self.show_activity_page)
        ob_layout.addWidget(ob_btn, 0, Qt.AlignmentFlag.AlignVCenter)
        self.overview_session_banner.setVisible(False)
        layout.addWidget(self.overview_session_banner)

        cards_grid = QGridLayout()
        cards_grid.setSpacing(10)

        card_tracks, self.card_tracks_metric, self.card_tracks_sub, self.card_tracks_btn = make_stat_card(
            '🎵 Local Tracks', '—', 'Resolution status', 'Link tracks →',
            lambda: self.nav.setCurrentRow(2)
        )
        cards_grid.addWidget(card_tracks, 0, 0)

        card_artists, self.card_artists_metric, self.card_artists_sub, self.card_artists_btn = make_stat_card(
            '👤 Album Artists', '—', 'Resolution status', 'Link artists →',
            lambda: self.nav.setCurrentRow(1)
        )
        cards_grid.addWidget(card_artists, 0, 1)

        card_releases, self.card_releases_metric, self.card_releases_sub, self.card_releases_btn = make_stat_card(
            '💿 Local Releases', '—', 'Resolution status', 'Clean tags →',
            lambda: (self.nav.setCurrentRow(5), self.tools_tabs.setCurrentIndex(1))
        )
        cards_grid.addWidget(card_releases, 0, 2)

        card_favs, self.card_favs_metric, self.card_favs_sub, self.card_favs_btn = make_stat_card(
            '⭐ Online Favourites', '—', 'Liked artists in account', 'View favourites →',
            lambda: self.nav.setCurrentRow(7)
        )
        cards_grid.addWidget(card_favs, 0, 3)

        card_gaps, self.card_gaps_metric, self.card_gaps_sub, self.card_gaps_btn = make_stat_card(
            '💿 Missing Releases', '—', 'Catalogue releases missing locally', 'View missing releases →',
            lambda: self.nav.setCurrentRow(3)
        )
        cards_grid.addWidget(card_gaps, 1, 0)

        card_queue, self.card_queue_metric, self.card_queue_sub, self.card_queue_btn = make_stat_card(
            '⬇️ Download Releases', '—', 'Releases ready to download / export', 'Open downloads →',
            lambda: self.nav.setCurrentRow(4)
        )
        cards_grid.addWidget(card_queue, 1, 1)

        card_hygiene, self.card_hygiene_metric, self.card_hygiene_sub, self.card_hygiene_btn = make_stat_card(
            '✨ Tag Hygiene', 'Standardised', 'Camelot keys, dates & stray tracks', 'Review tags →',
            lambda: (self.nav.setCurrentRow(5), self.tools_tabs.setCurrentIndex(1))
        )
        cards_grid.addWidget(card_hygiene, 1, 2)

        card_conn, self.card_conn_metric, self.card_conn_sub, self.card_conn_btn = make_stat_card(
            '🌐 Online Account', 'Ready' if self.demo_mode else 'Active', 'API & credentials session', 'Manage connections →',
            lambda: (self.nav.setCurrentRow(6), self.settings_tabs.setCurrentIndex(1))
        )
        cards_grid.addWidget(card_conn, 1, 3)
        self.overview_cards=[card_tracks,card_artists,card_releases,card_favs,card_gaps,card_queue,card_hygiene,card_conn]

        layout.addLayout(cards_grid)

        # Compatibility aliases for status labels
        self.hub_tag_status = self.card_hygiene_sub
        self.hub_artist_status = self.card_artists_sub
        self.hub_link_status = self.card_tracks_sub
        self.hub_gaps_status = self.card_gaps_sub
        self.hub_queue_status = self.card_queue_sub
        self.hub_status_labels = [self.hub_tag_status, self.hub_artist_status, self.hub_link_status, self.hub_gaps_status, self.hub_queue_status]

        # Compatibility metrics list (tested by ui_maintenance_smoke.py)
        self.metrics = []
        for _ in range(6):
            lbl = QLabel()
            lbl.setObjectName('metric')
            lbl.hide()
            layout.addWidget(lbl)
            self.metrics.append(lbl)

        def make_section_title(text):
            lbl = QLabel(text)
            font = lbl.font(); font.setBold(True); lbl.setFont(font)
            lbl.setStyleSheet("padding-top: 14px; padding-bottom: 6px; margin: 0px;")
            return lbl

        # Latest missing releases section
        layout.addWidget(make_section_title('Latest missing releases'))
        self.overview_missing = table(['Album artist', 'Release', 'Release date', 'Type', 'Tracks'])
        self.overview_missing.setMinimumHeight(180)
        for col in (2, 3, 4):
            self.overview_missing.horizontalHeader().setSectionResizeMode(col, QHeaderView.ResizeMode.ResizeToContents)
        self.overview_missing.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        self.overview_missing.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.Stretch)
        layout.addWidget(self.overview_missing, 1)

        # Latest downloaded releases section
        layout.addWidget(make_section_title('Latest downloaded releases'))
        self.overview_downloaded = table(['Album artist', 'Release', 'Downloaded date', 'Type', 'Tracks'])
        self.overview_downloaded.setMinimumHeight(180)
        for col in (2, 3, 4):
            self.overview_downloaded.horizontalHeader().setSectionResizeMode(col, QHeaderView.ResizeMode.ResizeToContents)
        self.overview_downloaded.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        self.overview_downloaded.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.Stretch)
        layout.addWidget(self.overview_downloaded, 1)

        # Retain hidden tables for background updaters and test compatibility
        self.overview_roots = table(['Library root', 'Last successful scan', 'Status'])
        self.overview_roots.hide()
        self.activity = table(['Date', 'Time', 'Result', 'Summary'])
        self.activity.hide()

        if self.demo_mode:
            notice = QLabel('Demo data is fictional and stored separately. Launch without --demo to scan your own library.')
            notice.setWordWrap(True); layout.addWidget(notice)

    def artists_page(self):
        layout = self.page('Link Artists', 'Match your local album artists with online artists to enable track linking and discography scans.')
        actions_box = QVBoxLayout()
        actions_box.setSpacing(6)

        online_row = QHBoxLayout()
        online_lbl = QLabel('Online:')
        online_lbl.setFixedWidth(52)
        online_lbl.setObjectName('muted')
        online_lbl.setStyleSheet('font-weight: 600; font-size: 11px;')
        online_row.addWidget(online_lbl)

        btn_match_sel = QPushButton('Match selected')
        btn_match_sel.setObjectName('primary')
        btn_match_sel.setEnabled(False)
        btn_match_sel.clicked.connect(self.find_candidates)
        self.btn_match_sel = btn_match_sel
        online_row.addWidget(btn_match_sel)

        btn_match_all = QPushButton('Match all')
        btn_match_all.clicked.connect(self.match_all)
        online_row.addWidget(btn_match_all)

        btn_link_id = QPushButton('Link artist ID…')
        btn_link_id.setToolTip('Link selected artist by numeric ID or web URL (artist or album)')
        btn_link_id.clicked.connect(self.link_id)
        online_row.addWidget(btn_link_id)

        btn_sync = QPushButton('Refresh release list')
        btn_sync.setToolTip('Refresh this artist’s album, EP, and single summaries. Track details are loaded later only when release linking needs them.')
        btn_sync.clicked.connect(self.sync_artist)
        self.artist_refresh_releases=btn_sync
        online_row.addWidget(btn_sync)
        online_row.addStretch()
        actions_box.addLayout(online_row)

        local_row = QHBoxLayout()
        local_lbl = QLabel('Local:')
        local_lbl.setFixedWidth(52)
        local_lbl.setObjectName('muted')
        local_lbl.setStyleSheet('font-weight: 600; font-size: 11px;')
        local_row.addWidget(local_lbl)

        btn_review = QPushButton('Review candidates')
        btn_review.clicked.connect(self.review_candidates)
        local_row.addWidget(btn_review)

        btn_extended = QPushButton('Extended review…')
        btn_extended.setToolTip('Find true releases by ISRC/recording for mis-tagged or stray tracks and retag/move them')
        btn_extended.clicked.connect(self.extended_artist_review)
        local_row.addWidget(btn_extended)

        btn_unlink = QPushButton('Unlink')
        btn_unlink.clicked.connect(self.unlink)
        local_row.addWidget(btn_unlink)
        local_row.addStretch()
        actions_box.addLayout(local_row)

        layout.addLayout(actions_box)
        self.artist_filter = QComboBox(); self.artist_filter.addItems(['All artists','Needs review','Unmatched','Errors','Auto-matched','Confirmed'])
        self.artist_filter.currentTextChanged.connect(self.filter_artists); layout.addWidget(self.artist_filter)
        self.batch_status = QLabel('Matching results are saved per artist. You can leave this page while a batch runs.')
        self.batch_status.setWordWrap(True); self.batch_status.setObjectName('muted'); layout.addWidget(self.batch_status)
        self.artist_table = table(['Local artist', 'Tracks', 'Singles', 'Albums / EPs', 'Identity', 'Evidence']); layout.addWidget(self.artist_table, 2)
        self.artist_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.artist_table.itemSelectionChanged.connect(self.local_details)
        self.artist_table.itemDoubleClicked.connect(lambda item:self.review_candidates())
        self.artist_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.artist_table.customContextMenuRequested.connect(self._artist_table_context_menu)
        self.local_table = table(['Local album / release', 'Local tracks', 'Release type', 'Date']); layout.addWidget(self.local_table, 1)
        self.local_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.local_table.customContextMenuRequested.connect(self._local_table_context_menu)

    def link_releases_page(self):
        layout = self.page('Link Releases', 'Match your local releases and tracks to the online catalogue and save verified links.')
        self.link_page_widget = layout.parentWidget()

        # Drive selector (retained in memory for background operations)
        self.link_releases_root = QComboBox()
        self.link_releases_root.hide()

        # Action buttons rows
        action_row = QHBoxLayout()
        action_row.setSpacing(8)
        self.link_start = QPushButton('Start / resume linking')
        self.link_start.setObjectName('primary')
        self.link_start.clicked.connect(self.start_linking)
        self.link_start.setToolTip('Checks only unresolved tracks in the active library. Existing links are retained; saved unresolved results are reused.')
        action_row.addWidget(self.link_start)

        self.link_pause = QPushButton('Pause linking')
        self.link_pause.setEnabled(False)
        self.link_pause.clicked.connect(self.pause_linking)
        action_row.addWidget(self.link_pause)

        self.link_choose_btn = QPushButton('Choose match for selected…')
        self.link_choose_btn.clicked.connect(self.tools_choose_album)
        self.link_choose_btn.setToolTip('Pick between verified online album/artist placements for the selected track')
        action_row.addWidget(self.link_choose_btn)

        link_rel_id_btn = QPushButton('Link release ID…')
        link_rel_id_btn.setToolTip('Manually link selected tracks to a specific release ID or web URL')
        link_rel_id_btn.clicked.connect(self.link_release_id)
        action_row.addWidget(link_rel_id_btn)
        action_row.addStretch()
        layout.addLayout(action_row)

        action_row2 = QHBoxLayout()
        action_row2.setSpacing(8)
        recheck_editions_btn = QPushButton('Recheck multiple editions')
        recheck_editions_btn.setToolTip('Re-evaluate tracks that had too many possible editions or choices')
        recheck_editions_btn.clicked.connect(self.recheck_multiple_editions)
        action_row2.addWidget(recheck_editions_btn)
        recheck_unlinked=QPushButton('Recheck unlinked tracks')
        recheck_unlinked.setToolTip('Retry unresolved tracks only, including previously unsuccessful searches. Linked and ignored files are excluded.')
        recheck_unlinked.clicked.connect(self.recheck_unlinked_tracks)
        action_row2.addWidget(recheck_unlinked)
        self.link_missing_btn=QPushButton('Queue missing album tracks…'); self.link_missing_btn.clicked.connect(self.queue_link_missing); action_row2.addWidget(self.link_missing_btn)

        review = QPushButton('Review saved choices…')
        review.clicked.connect(self.review_saved_links)
        action_row2.addWidget(review)

        retry = QPushButton('Recheck selected')
        retry.clicked.connect(lambda: self.start_linking(recheck=True))
        action_row2.addWidget(retry)
        action_row2.addStretch()
        layout.addLayout(action_row2)

        self.link_status = QLabel('Ready · saved results resume across restarts. Artist matches can be reviewed in Link Artists.')
        self.link_status.hide()

        # Search and filter bar
        filter_bar = QHBoxLayout()
        self.link_search = QLineEdit()
        self.link_search.setPlaceholderText('Find a file, artist or album…')
        self.link_search.textChanged.connect(self.filter_link_table)
        filter_bar.addWidget(self.link_search, 1)

        self.link_filter = QComboBox()
        self.link_filter.addItems(['All files', 'Needs attention', 'Unlinked tracks', 'Linked tracks', 'Too many editions / Needs choice', 'Ignored files'])
        self.link_filter.currentTextChanged.connect(self.filter_link_table)
        filter_bar.addWidget(self.link_filter)
        layout.addLayout(filter_bar)

        self.link_table = table(['Local File', 'Needs attention', 'Online release match', 'Recording evidence', 'Result', 'Artist', 'Release'], virtual=True)
        self.link_table.horizontalHeader().moveSection(5,0)
        self.link_table.horizontalHeader().moveSection(6,1)
        self.link_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.link_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.link_table.customContextMenuRequested.connect(self._link_table_context_menu)
        self.link_table.itemSelectionChanged.connect(self.update_link_detail)
        layout.addWidget(self.link_table, 1)

        self.link_detail = QPlainTextEdit()
        self.link_detail.setReadOnly(True)
        self.link_detail.setMinimumHeight(100)
        self.link_detail.setMaximumHeight(155)
        self.link_detail.setPlaceholderText('Select a file to see its catalogue match options, DJ analysis and evidence.')
        self.link_detail.hide()
        layout.addWidget(self.link_detail)

        self.link_table_status = QLabel()
        self.link_table_status.setWordWrap(True)
        layout.addWidget(self.link_table_status)

        footer = QHBoxLayout()
        inspect = QPushButton('Inspect all tags…')
        inspect.clicked.connect(self.tools_inspect_tags)
        footer.addWidget(inspect)
        self.link_details_btn = QPushButton('Show details')
        self.link_details_btn.setCheckable(True)
        self.link_details_btn.toggled.connect(lambda visible: (self.link_detail.setVisible(visible), self.link_details_btn.setText('Hide details' if visible else 'Show details')))
        footer.addWidget(self.link_details_btn)
        footer.addStretch()
        layout.addLayout(footer)

        self.link_releases_root.currentIndexChanged.connect(self._on_link_releases_root_changed)
        self.link_plan = []

    def _on_link_releases_root_changed(self, idx):
        if hasattr(self, 'tools_root') and self.tools_root.currentIndex() != idx:
            self.tools_root.blockSignals(True)
            self.tools_root.setCurrentIndex(idx)
            self.tools_root.blockSignals(False)
        if hasattr(self, 'active_drive_combo') and self.active_drive_combo.currentIndex() != idx:
            self.active_drive_combo.blockSignals(True)
            self.active_drive_combo.setCurrentIndex(idx)
            self.active_drive_combo.blockSignals(False)
        self.render_link_releases()

    def _on_roots_selection_changed(self):
        row = source_row(self.roots,self.roots.currentRow())
        if row >= 0 and hasattr(self, 'active_drive_combo') and row < self.active_drive_combo.count():
            if self.active_drive_combo.currentIndex() != row:
                self.active_drive_combo.setCurrentIndex(row)

    def _on_active_drive_changed(self, idx):
        if idx < 0: return
        if hasattr(self, 'roots') and self.roots.currentRow() != idx and 0 <= idx < self.roots.rowCount():
            self.roots.blockSignals(True)
            self.roots.selectRow(next((n for n in range(self.roots.rowCount()) if source_row(self.roots,n)==idx),idx))
            self.roots.blockSignals(False)
        if hasattr(self, 'tools_root') and self.tools_root.currentIndex() != idx:
            self.tools_root.setCurrentIndex(idx)
        if hasattr(self, 'link_releases_root') and self.link_releases_root.currentIndex() != idx:
            self.link_releases_root.setCurrentIndex(idx)

    def _on_tools_root_changed(self, idx):
        if hasattr(self, 'active_drive_combo') and self.active_drive_combo.currentIndex() != idx:
            self.active_drive_combo.blockSignals(True)
            self.active_drive_combo.setCurrentIndex(idx)
            self.active_drive_combo.blockSignals(False)
        if hasattr(self, 'link_releases_root') and self.link_releases_root.currentIndex() != idx:
            self.link_releases_root.blockSignals(True)
            self.link_releases_root.setCurrentIndex(idx)
            self.link_releases_root.blockSignals(False)
        self.invalidate_tools_plan()

    def _on_nav_page_changed(self, index):
        if index >= 0:
            if hasattr(self, 'activity_nav_btn'):
                self.activity_nav_btn.setChecked(False)
                self.activity_nav_btn.setProperty('selected', False)
                self.activity_nav_btn.style().unpolish(self.activity_nav_btn)
                self.activity_nav_btn.style().polish(self.activity_nav_btn)
            self.stack.setCurrentIndex(index)
            if index in (14,15,16) and self._view_data:self.update_category_cards(self._view_data)
            if index == 2:
                self.render_link_releases()
            elif index == 5:
                if getattr(self,'_prepared_mode',None) is None:self.invalidate_tools_plan()
                else:self.render_tools_plan()
            elif index == 7:
                self.render_favourites_page()
            elif index == 9:
                self.refresh_downloaded_releases()

    def show_activity_page(self):
        self.nav.setCurrentRow(8)

    def activity_page(self):
        page = QWidget()
        layout = QVBoxLayout(page)
        layout.setContentsMargins(0, 4, 0, 0)
        layout.setSpacing(14)

        header = QVBoxLayout()
        header.setSpacing(4)
        title = QLabel('Activity')
        title.setObjectName('title')
        header.addWidget(title)
        subtitle = QLabel('Session event log, diagnostics, and running operations.')
        subtitle.setObjectName('muted')
        header.addWidget(subtitle)
        layout.addLayout(header)

        # Status card
        status_card = QFrame()
        status_card.setObjectName('overview_card')
        status_card.setStyleSheet('''
            QFrame#overview_card {
                background-color: rgba(128, 128, 128, 0.08);
                border: 1px solid rgba(128, 128, 128, 0.18);
                border-radius: 8px;
                padding: 4px;
            }
        ''')
        status_layout = QHBoxLayout(status_card)
        status_layout.setContentsMargins(14, 10, 14, 10)
        status_layout.setSpacing(12)

        self.activity_job_icon = QLabel('⚡')
        font = self.activity_job_icon.font(); font.setPointSize(16); self.activity_job_icon.setFont(font)
        status_layout.addWidget(self.activity_job_icon)

        info_layout = QVBoxLayout()
        info_layout.setSpacing(2)
        self.activity_job_heading = QLabel('No background tasks currently running')
        font = self.activity_job_heading.font(); font.setBold(True); self.activity_job_heading.setFont(font)
        info_layout.addWidget(self.activity_job_heading)
        self.activity_job_detail = QLabel('Ready for operations.')
        self.activity_job_detail.setObjectName('muted')
        info_layout.addWidget(self.activity_job_detail)
        self.activity_job_eta = QLabel('')
        self.activity_job_eta.setStyleSheet('font-size: 11px; font-weight: 500; color: #007AFF;')
        self.activity_job_eta.setVisible(False)
        info_layout.addWidget(self.activity_job_eta)
        status_layout.addLayout(info_layout, 1)

        self.activity_pause_link_btn = QPushButton('Pause background linking')
        self.activity_pause_link_btn.clicked.connect(self.toggle_link_pause)
        self.activity_pause_link_btn.setVisible(False)
        status_layout.addWidget(self.activity_pause_link_btn)

        self.activity_cancel_btn = QPushButton('Cancel current job')
        self.activity_cancel_btn.setEnabled(False)
        self.activity_cancel_btn.clicked.connect(self.cancel_job)
        status_layout.addWidget(self.activity_cancel_btn)

        layout.addWidget(status_card)

        # Filter bar
        bar = QHBoxLayout()
        bar.setSpacing(10)
        self.activity_filter_input = QLineEdit()
        self.activity_filter_input.setPlaceholderText('Filter log messages…')
        self.activity_filter_input.setClearButtonEnabled(True)
        bar.addWidget(self.activity_filter_input, 1)

        if not hasattr(self, 'activity_autoscroll'):
            self.activity_autoscroll = QCheckBox('Auto-scroll activity log')
            self.activity_autoscroll.setChecked(True)

        self.activity_clear_btn = QPushButton('Clear log')
        self.activity_clear_btn.clicked.connect(self._clear_activity_log)
        bar.addWidget(self.activity_clear_btn)

        self.activity_copy_btn = QPushButton('Copy log')
        self.activity_copy_btn.clicked.connect(self._copy_activity_log)
        bar.addWidget(self.activity_copy_btn)

        layout.addLayout(bar)

        # Log viewer
        font = QFontDatabase.systemFont(QFontDatabase.SystemFont.FixedFont)
        font.setPointSize(12)
        self.activity_log.setFont(font)
        layout.addWidget(self.activity_log, 1)

        self.activity_filter_input.textChanged.connect(self._filter_activity_log)
        return page

    def _filter_activity_log(self):
        query = self.activity_filter_input.text().strip().lower() if hasattr(self, 'activity_filter_input') else ''
        lines = getattr(self, '_raw_activity_lines', [])
        if not lines:
            text = self.activity_log.toPlainText()
            if text:
                lines = text.splitlines()
                self._raw_activity_lines = list(lines)
        if not query:
            filtered = lines
        else:
            filtered = [l for l in lines if query in l.lower()]
        self.activity_log.setPlainText('\n'.join(filtered))
        if getattr(self, 'activity_autoscroll', None) and self.activity_autoscroll.isChecked():
            from PySide6.QtGui import QTextCursor
            self.activity_log.moveCursor(QTextCursor.MoveOperation.End)

    def _clear_activity_log(self):
        self._raw_activity_lines = []
        self.activity_log.clear()

    def _copy_activity_log(self):
        text = self.activity_log.toPlainText()
        QApplication.clipboard().setText(text)
        self.activity_copy_btn.setText('Copied!')
        QTimer.singleShot(1500, lambda: self.activity_copy_btn.setText('Copy log') if hasattr(self, 'activity_copy_btn') else None)

    def render_link_releases(self):
        if not hasattr(self, 'link_table'): return
        root = self.link_releases_root.currentData() if hasattr(self, 'link_releases_root') and self.link_releases_root.currentData() else (self.tools_root.currentData() if hasattr(self, 'tools_root') else None)
        if not root:
            fill(self.link_table, [], keys=[], row_types={})
            self.link_table_status.setText('Choose a library to view track linking status.')
            return
        from .linking import attach_links
        rows = []
        plan = self.tools_snapshots.get(root, [])
        if not plan:
            plan=self._load_cached_snapshot(root)
            self.tools_snapshots[root]=plan
        plan = attach_links(plan, self.store, self.market)
        from .link_statistics import link_statistics
        active=link_statistics(self.store,self.market,root)['active']
        for row in plan:row['linked_ids']=active.get(row['path'],{})
        self.link_plan = plan
        ignored_paths = self.store.ignored_local_files()
        row_types = {}
        for idx, row in enumerate(plan):
            if row['path'] in ignored_paths:
                row_types[idx] = 'ignored'
            try:
                rel_path = str(Path(row['path']).relative_to(row['root']))
            except Exception:
                rel_path = Path(row['path']).name
            choice = row.get('catalogue_choice')
            opts = row.get('catalogue_options') or []
            linked = row.get('linked_ids') or {}
            if linked:
                status_text = 'Linked · Chosen match' if row.get('manual') else 'Linked · Ready'
            elif opts:
                status_text = 'Review release structure' if not any(o.get('structure',{}).get('compatible') for o in opts) else f"Needs choice ({len(opts)} options)"
            elif row.get('blocked'):
                status_text = str(row['blocked'])
            else:
                status_text = 'Unlinked · Not in online catalogue'
            if row['path'] in ignored_paths:
                status_text = f"[Ignored] {status_text}"
            if row.get('mqa_audit',{}).get('detected'):
                status_text += ' · MQA replacement available'
            if choice or linked:
                from .linking import online_match_label
                match_text = online_match_label(row)
            elif opts:
                match_text = f"{len(opts)} candidates · {sum(o.get('structure',{}).get('compatible',False) for o in opts)} structurally compatible · Choose match…"
            else:
                match_text = '—'
            if choice:
                evidence_text = f"Track {choice.get('track_id', '')} · {choice.get('evidence', '')}"
            elif row.get('catalogue_note'):
                evidence_text = row['catalogue_note']
            else:
                evidence_text = '—'
            tags=row.get('tags',{})
            rows.append((rel_path, status_text, match_text, evidence_text, 'Saved in database', ', '.join(tags.get('albumartist',[])), ', '.join(tags.get('album',[]))))
        fill(self.link_table, rows, keys=[r['path'] for r in plan], row_types=row_types)
        self.link_table.setColumnHidden(2, False)
        self.link_table.setColumnHidden(3, False)
        self.link_table.setColumnHidden(4, True)
        from .link_statistics import link_statistics,summary
        self.link_table_status.setText(summary(link_statistics(self.store,self.market))+' · All libraries')
        self.filter_link_table()

    def filter_link_table(self):
        if not hasattr(self, 'link_table') or not hasattr(self, 'link_plan'): return
        query = self.link_search.text().casefold() if hasattr(self, 'link_search') else ''
        filter_text = self.link_filter.currentText() if hasattr(self, 'link_filter') else 'All files'
        from .link_statistics import link_statistics,matches_link_filter
        active=link_statistics(self.store,self.market)['active']
        ignored_paths = self.store.ignored_local_files()
        by_path = {r['path']: r for r in self.link_plan}
        for index in range(self.link_table.rowCount()):
            key = self.link_table.item(index,0).data(Qt.ItemDataRole.UserRole)
            row = by_path.get(key, {})
            text = ' '.join(self.link_table.item(index, col).text() for col in range(self.link_table.columnCount()) if self.link_table.item(index, col))
            hidden = bool(query and query not in text.casefold())
            hidden |= not matches_link_filter(row,filter_text,active,ignored_paths)
            self.link_table.setRowHidden(index, hidden)

    def queue_link_missing(self):
        selected=self.link_selected()
        if not selected or self.worker:return
        if self.demo_mode:return self.demo_notice()
        row=selected[0];choice=row.get('catalogue_choice');ids=row.get('linked_ids') or {}
        ident=str((choice or {}).get('id') or ids.get('album_id') or '')
        if not ident:
            QMessageBox.information(self,'Link a release first','Choose a verified release match before queueing its missing tracks.');return
        def work(cancel,progress):
            from .release_matching import group_key,structure,release_folder,position
            from .library_workflows import inspect_snapshot
            import time
            release=self.store.preferences(f'tag-review:{self.market}:{ident}')
            if not release.get('tracks') or time.time()-release.get('tag_checked_at',0)>86400:
                release=self.api(cancel,progress).album_tag_details({'id':ident})
                self.store.save_preferences(f'tag-review:{self.market}:{ident}',dict(release,tag_checked_at=time.time()))
            snapshot=inspect_snapshot(row['root'],[],cancel=cancel,progress=progress,layout=self.store.preferences('organisation'))
            group=[r for r in snapshot if group_key(r)==group_key(row)]
            info=structure(group,release)
            if not group or not info['compatible']:raise CatalogueError('Local positions or release totals differ; review the chosen edition first')
            if not info['missing']:raise CatalogueError('No missing local tracks in this release')
            destination=dict(root=row['root'],album_relative=str(release_folder(row).relative_to(row['root'])),
                             disc_dirs={str(position(r)[0]):str(Path(r['path']).parent.relative_to(release_folder(row))) for r in group})
            return dict(release,existing_destination=destination),info['missing']
        def done(result):
            release,missing=result
            self.track_selection_dialog(release,missing)
        self.job(work,done,label='Check missing tracks in linked album')

    def update_link_detail(self):
        if not hasattr(self, 'link_table') or not hasattr(self, 'link_plan'): return
        selected = self.link_selected()
        if not selected:
            self.link_detail.setPlainText('Select a file to see its catalogue match options, DJ analysis and evidence.')
            return
        row = selected[0]
        from .maintenance import first
        tags = row.get('tags', {})
        lines = [
            f"File: {Path(row['path']).name}",
            f"Local Album Artist: {first(tags, 'albumartist') or '—'}",
            f"Local Track Artist: {first(tags, 'artist') or '—'}",
            f"Local Album: {first(tags, 'album') or '—'}",
            f"Local Title: {first(tags, 'title') or '—'}",
            ""
        ]
        choice = row.get('catalogue_choice')
        linked = row.get('linked_ids') or {}
        if choice:
            lines.append(f"🌐 Linked Online Album: {choice.get('album')} [{choice.get('id')}]")
            lines.append(f"🌐 Linked Online Track: {choice.get('title', '—')} [ID: {choice.get('track_id', '—')}]")
            if choice.get('evidence'):
                lines.append(f"   Evidence: {choice.get('evidence')}")
        elif linked:
            lines.append(f"🌐 Linked Online Album ID: {linked.get('album_id', '—')}")
            lines.append(f"🌐 Linked Online Track ID: {linked.get('track_id', '—')}")
        equivalent=row.get('equivalent_ids') or (choice or {}).get('equivalent_placements',[])
        if len(equivalent)>1:
            pairs=', '.join(f"album {p.get('album_id')} / track {p.get('track_id')}" for p in equivalent)
            lines.append(f"Equivalent catalogue IDs retained: {pairs}")
        if row.get('mqa_audit',{}).get('detected'):
            lines.append(f"MQA audit: {row['mqa_audit'].get('status')} · replacement can be queued from Prepare Library → MQA Audit")
        opts = row.get('catalogue_options') or []
        dj_checks = row.get('dj_checks', [])
        if opts:
            lines.append(f"📋 Catalogue Options ({len(opts)} available):")
            for i, opt in enumerate(opts, 1):
                ver = " [Recording verified]" if opt.get('recording_verified') else (" [Whole release verified]" if opt.get('whole_release_verified') else "")
                lines.append(f"   {i}. {opt.get('artist')} — {opt.get('album')} ({opt.get('year', '—')}) [{opt.get('type', 'ALBUM')} · ID {opt.get('id')}]{ver}")
                dj = opt.get('dj') or (dj_checks[i - 1] if i - 1 < len(dj_checks) else None)
                if dj:
                    bpm = dj.get('bpm')
                    key = dj.get('key')
                    bpm_str = ', '.join(bpm) if isinstance(bpm, list) else (str(bpm) if bpm else '—')
                    key_str = ', '.join(key) if isinstance(key, list) else (str(key) if key else '—')
                    lines.append(f"      • DJ Analysis: BPM {bpm_str} · Key {key_str}")
        elif dj_checks:
            for check in dj_checks:
                bpm = check.get('bpm')
                key = check.get('key')
                bpm_str = ', '.join(bpm) if isinstance(bpm, list) else (str(bpm) if bpm else '—')
                key_str = ', '.join(key) if isinstance(key, list) else (str(key) if key else '—')
                lines.append(f"   • DJ Analysis: BPM {bpm_str} · Key {key_str}")
        self.link_detail.setPlainText('\n'.join(lines))

    def link_selected(self):
        if not hasattr(self, 'link_table') or not hasattr(self, 'link_plan'): return []
        indexes = self.link_table.selectionModel().selectedRows()
        if not indexes: return []
        by_path = {r.get('path'): r for r in self.link_plan if r.get('path')}
        model = self.link_table.model()
        selected = []
        for idx in indexes:
            r = idx.row()
            if self.link_table.isRowHidden(r):
                continue
            path = None
            if hasattr(model, 'keys') and 0 <= r < len(model.keys):
                path = model.keys[r]
            elif self.link_table.item(r, 0):
                path = self.link_table.item(r, 0).data(Qt.ItemDataRole.UserRole)
            if path and path in by_path:
                selected.append(by_path[path])
            elif 0 <= r < len(self.link_plan):
                selected.append(self.link_plan[r])
        return selected

    @staticmethod
    def _select_context_row(table,pos):
        """Make the row under a right click the action target for every table."""
        index=table.indexAt(pos)
        if not index.isValid():return -1
        row=index.row()
        selected={item.row() for item in table.selectionModel().selectedRows()}
        if row not in selected:table.selectRow(row)
        table.selectionModel().setCurrentIndex(index,QItemSelectionModel.SelectionFlag.NoUpdate)
        return row

    def _link_table_context_menu(self, pos):
        if not hasattr(self, 'link_table'): return
        row = self._select_context_row(self.link_table,pos)
        if row < 0:return
        model = self.link_table.model()
        path = None
        if hasattr(model, 'keys') and 0 <= row < len(model.keys):
            path = model.keys[row]
        elif self.link_table.item(row, 0):
            path = self.link_table.item(row, 0).data(Qt.ItemDataRole.UserRole)
        by_path = {r.get('path'): r for r in self.link_plan if r.get('path')}
        plan_row = by_path.get(path)
        if not plan_row and 0 <= row < len(self.link_plan):
            plan_row = self.link_plan[row]
            path = plan_row.get('path')
        if not plan_row: return
        from .context_actions import context_menu
        menu=context_menu(self,path=path,metadata=plan_row,choose=self.tools_choose_album if plan_row.get('catalogue_options') else None)
        act_link_rel = menu.addAction('Link release ID…')
        act_link_rel.triggered.connect(self.link_release_id)
        act_recheck = menu.addAction('Search matches for selected tracks')
        act_recheck.triggered.connect(self.recheck_selected_tracks)
        selected_paths={r['path'] for r in self.link_selected()}
        if selected_paths:
            ignored_paths=self.store.ignored_local_files()
            restore=selected_paths.issubset(ignored_paths)
            action=menu.addAction(('Stop ignoring' if restore else 'Ignore')+f' selected tracks ({len(selected_paths)})')
            action.triggered.connect(lambda:self.set_link_selection_ignored(selected_paths,not restore))
        menu.exec(self.link_table.viewport().mapToGlobal(pos))

    def set_link_selection_ignored(self,paths,ignored):
        self.store.set_local_files_ignored(paths,ignored)
        # Filtering existing rows avoids rebuilding/rehydrating the entire library.
        self.filter_link_table()
        self.log(f"{'Ignored' if ignored else 'Restored'} {len(paths):,} selected tracks · local files unchanged")

    def artist_state(self, name, cached_mapping=None, cached_review=None):
        mappings = self.store.rows('SELECT * FROM mappings WHERE artist=?',(name,)) if cached_mapping is None else ([cached_mapping] if cached_mapping else [])
        reviews = self.store.rows('SELECT * FROM match_reviews WHERE artist=?',(name,)) if cached_review is None else ([cached_review] if cached_review else [])
        mapping = mappings[0] if mappings else {}
        if mapping.get('manual'):
            return mapping['status'], json.loads(mapping['evidence'])
        if reviews and reviews[0]['status']=='error':
            return 'error', reviews[0]['error']
        if mapping:
            return mapping['status'], json.loads(mapping['evidence'])
        return 'unmatched', 'Not matched yet · select artists or use Match all'

    @staticmethod
    def match_label(state):
        return {'auto':'Auto-matched','review':'Needs review','unmatched':'Unmatched','error':'Error','confirmed':'Confirmed','unlinked':'Manually unlinked'}.get(state,state)

    def update_artist_row(self, name):
        for row in range(self.artist_table.rowCount()):
            artist,tracks=self.artist_rows[source_row(self.artist_table,row)]
            if artist==name:
                state,evidence=self.artist_state(name)
                self.artist_table.setItem(row,4,QTableWidgetItem(self.match_label(state)))
                item=QTableWidgetItem(str(evidence));item.setToolTip(str(evidence));self.artist_table.setItem(row,5,item)
                break
        self.filter_artists()

    def filter_artists(self):
        states={'Needs review':{'review'},'Unmatched':{'unmatched','unlinked'},'Errors':{'error'},'Auto-matched':{'auto'},'Confirmed':{'confirmed'}}
        allowed=states.get(self.artist_filter.currentText())
        for row in range(self.artist_table.rowCount()):
            item=self.artist_table.item(row,4)
            self.artist_table.setRowHidden(row, bool(allowed and item and item.text() not in {self.match_label(state) for state in allowed}))
        if hasattr(self, 'btn_match_sel'):
            has_sel = bool(self.artist_table.selectionModel() and self.artist_table.selectionModel().selectedRows())
            self.btn_match_sel.setEnabled(has_sel)

    def show_favourites_dialog(self):
        self.nav.setCurrentRow(7)
        self.render_favourites_page()

    def _update_card_favs_metric(self, favs=None):
        if not hasattr(self, 'card_favs_metric'):
            return
        if favs is None:
            favs = getattr(self, 'cached_tidal_favourites', None)
        if favs is None and hasattr(self, 'store'):
            try:
                rows = self.store.rows('SELECT payload FROM favourite_artists ORDER BY fetched DESC LIMIT 1')
                if rows and rows[0].get('payload'):
                    favs = json.loads(rows[0]['payload'])
                    self.cached_tidal_favourites = favs
            except Exception:
                pass
        if favs is not None:
            from .favourites_view import favourite_rows
            rows = favourite_rows(favs, self.store.artists(), self.store.linked_mappings())
            in_local = sum(r[1] == 'In library' for r in rows)
            missing = sum(r[1] == 'Missing locally' for r in rows)
            self.card_favs_metric.setText(f'{in_local + missing:,} favourites')
            self.card_favs_sub.setText(f'{in_local:,} in library · {missing:,} missing locally')
        else:
            self.card_favs_metric.setText('Not loaded')
            self.card_favs_sub.setText('Refresh favourites in Connections')
        card=getattr(self,'category_cards',{}).get(('Link Catalogue',2))
        if card:
            card.setText('⭐  Favourite Artists\n'+self.card_favs_metric.text())
            card.description_label.setText(self.card_favs_sub.text())

    def refresh_favourites_cache(self):
        if self.worker: return
        if self.demo_mode:
            mock_favs = [
                dict(id='900001', name='North Assembly'),
                dict(id='900099', name='Solar Fields'),
                dict(id='900100', name='Carbon Based Lifeforms')
            ]
            self.cached_tidal_favourites = mock_favs
            self._update_card_favs_metric(mock_favs)
            self.render_favourites_page()
            self.log('Refreshed Online favourites cache (demo simulation).')
            return
        def work(cancel, progress):
            return self.load_favourites(cancel, progress, refresh=True)
        def done(favs):
            self.cached_tidal_favourites = favs
            self._update_card_favs_metric(favs)
            self.render_favourites_page()
            self.log(f'Refreshed Online favourites cache ({len(favs)} artists).')
        self.job(work, done, label='Refresh Online favourites')

    def favourites_page(self):
        page = QWidget()
        layout = QVBoxLayout(page)
        layout.setContentsMargins(0, 4, 0, 0)
        layout.setSpacing(14)

        header = QVBoxLayout()
        header.setSpacing(4)
        title = QLabel('Favourite Artists')
        title.setObjectName('title')
        header.addWidget(title)
        subtitle = QLabel('Compare your liked online artists with your local music library.')
        subtitle.setObjectName('muted')
        header.addWidget(subtitle)
        layout.addLayout(header)

        bar = QHBoxLayout()
        bar.setSpacing(10)
        self.favs_search = QLineEdit()
        self.favs_search.setPlaceholderText('Filter artists…')
        self.favs_search.setClearButtonEnabled(True)
        self.favs_search.textChanged.connect(self._filter_favourites_tables)
        bar.addWidget(self.favs_search, 2)

        self.favs_filter_combo = QComboBox()
        self.favs_filter_combo.addItems(['All artists', 'Missing locally', 'In library', 'Local only'])
        self.favs_filter_combo.currentTextChanged.connect(self._filter_favourites_tables)
        bar.addWidget(self.favs_filter_combo)

        self.favs_refresh_btn = QPushButton('Refresh online favourites')
        self.favs_refresh_btn.clicked.connect(self.refresh_favourites_cache)
        bar.addWidget(self.favs_refresh_btn)

        self.favs_match_btn = QPushButton('Match in Link Artists')
        self.favs_match_btn.clicked.connect(self._favs_match_selected)
        bar.addWidget(self.favs_match_btn)

        layout.addLayout(bar)

        self.favs_table = table(['Artist', 'Category', 'Local Tracks', 'Online ID', 'Status'])
        self.favs_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.favs_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.favs_table.customContextMenuRequested.connect(self._favs_table_context_menu)
        layout.addWidget(self.favs_table, 1)

        self.favs_status = QLabel('')
        self.favs_status.setObjectName('muted')
        layout.addWidget(self.favs_status)

        # Retain table aliases for backwards compatibility
        self.favs_tabs = QTabWidget()
        self.favs_missing_table = self.favs_table
        self.favs_both_table = self.favs_table
        self.favs_local_table = self.favs_table

        self._favs_all_data = []
        self._favs_missing_data = []
        self._favs_both_data = []
        self._favs_local_data = []
        return page

    def render_favourites_page(self):
        favs = getattr(self, 'cached_tidal_favourites', None)
        if favs is None:
            if self.demo_mode:
                favs = [
                    dict(id='900001', name='North Assembly'),
                    dict(id='900099', name='Solar Fields'),
                    dict(id='900100', name='Carbon Based Lifeforms')
                ]
                self.cached_tidal_favourites = favs
            else:
                rows = self.store.rows('SELECT payload FROM favourite_artists ORDER BY fetched DESC LIMIT 1')
                if rows and rows[0].get('payload'):
                    try:
                        favs = json.loads(rows[0]['payload'])
                        self.cached_tidal_favourites = favs
                    except Exception:
                        favs = None
        if favs is None:
            favs = []

        from .favourites_view import favourite_rows
        all_data = favourite_rows(favs, self.store.artists(), self.store.linked_mappings())
        missing_locally = [r for r in all_data if r[1] == 'Missing locally']
        in_both = [r for r in all_data if r[1] == 'In library']
        local_only = [r for r in all_data if r[1] == 'Local only']
        self._update_card_favs_metric(favs)

        self._favs_missing_data = missing_locally
        self._favs_both_data = in_both
        self._favs_local_data = local_only
        self._favs_all_data = all_data

        if hasattr(self, 'favs_filter_combo'):
            self.favs_filter_combo.blockSignals(True)
            cur_idx = self.favs_filter_combo.currentIndex()
            self.favs_filter_combo.clear()
            self.favs_filter_combo.addItems([
                f'All artists ({len(all_data):,})',
                f'Missing locally ({len(missing_locally):,})',
                f'In library ({len(in_both):,})',
                f'Local only ({len(local_only):,})'
            ])
            self.favs_filter_combo.setCurrentIndex(max(0, cur_idx))
            self.favs_filter_combo.blockSignals(False)

        self._filter_favourites_tables()

    def _filter_favourites_tables(self):
        if not hasattr(self, 'favs_table') or not hasattr(self, '_favs_all_data'):
            return
        query = self.favs_search.text().strip().lower() if hasattr(self, 'favs_search') else ''
        cat_text = self.favs_filter_combo.currentText() if hasattr(self, 'favs_filter_combo') else 'All'

        target_cat = None
        if 'Missing locally' in cat_text:
            target_cat = 'Missing locally'
        elif 'In library' in cat_text:
            target_cat = 'In library'
        elif 'Local only' in cat_text:
            target_cat = 'Local only'

        sorting = self.favs_table.isSortingEnabled()
        self.favs_table.setSortingEnabled(False)
        self.favs_table.setRowCount(0)
        displayed = 0
        for row in self._favs_all_data:
            artist, cat, tracks, tid, status = row
            if target_cat and cat != target_cat:
                continue
            if query and not any(query in str(c).lower() for c in row):
                continue
            r_idx = self.favs_table.rowCount()
            self.favs_table.insertRow(r_idx)
            for c_idx, val in enumerate(row):
                it = QTableWidgetItem(str(val))
                if c_idx == 2 and str(val).isdigit():
                    it.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
                self.favs_table.setItem(r_idx, c_idx, it)
            displayed += 1

        self.favs_table.setSortingEnabled(sorting)
        if hasattr(self, 'favs_status'):
            self.favs_status.setText(f'{displayed:,} artists visible ({len(self._favs_all_data):,} total)')

    def _favs_match_selected(self):
        r = self.favs_table.currentRow() if hasattr(self, 'favs_table') else -1
        if r >= 0 and self.favs_table.item(r, 0):
            artist_name = self.favs_table.item(r, 0).text()
            self.nav.setCurrentRow(1)
            if hasattr(self, 'artist_table'):
                for i in range(self.artist_table.rowCount()):
                    it = self.artist_table.item(i, 0)
                    if it and it.text().strip().lower() == artist_name.strip().lower():
                        self.artist_table.setCurrentCell(i, 0)
                        break

    def _favs_table_context_menu(self, pos):
        r=self._select_context_row(self.favs_table,pos)
        if r < 0: return
        name = self.favs_table.item(r, 0).text() if self.favs_table.item(r, 0) else ''
        tid = self.favs_table.item(r, 3).text() if self.favs_table.item(r, 3) else ''
        menu = QMenu(self)
        for artist_id in (part.strip() for part in tid.split(',')):
            if not artist_id.isdecimal(): continue
            act_open = menu.addAction('Open on web')
            act_open.triggered.connect(lambda checked=False, ident=artist_id: QDesktopServices.openUrl(QUrl(f'https://tidal.com/browse/artist/{ident}')))
        act_link = menu.addAction('View in Link Artists')
        act_link.triggered.connect(self._favs_match_selected)
        act_copy = menu.addAction('Copy artist name')
        act_copy.triggered.connect(lambda: QApplication.clipboard().setText(name))
        menu.exec(self.favs_table.viewport().mapToGlobal(pos))

    def _favs_missing_context_menu(self, pos):
        self._favs_table_context_menu(pos)

    def _favs_both_context_menu(self, pos):
        self._favs_table_context_menu(pos)

    def _favs_local_context_menu(self, pos):
        self._favs_table_context_menu(pos)

    def missing_page(self):
        layout = self.page('Missing Releases', 'Discover and acquire new music and catalogue gaps for your linked artists.')
        actions_bar = QGridLayout()
        actions_bar.setHorizontalSpacing(8);actions_bar.setVerticalSpacing(8)

        scan_btn = QPushButton('Scan for new releases');self.scan_releases_button=scan_btn
        scan_btn.setObjectName('primary')
        scan_btn.setToolTip('Fetch the latest discographies from Online for all linked artists')
        scan_btn.clicked.connect(self.sync_all_catalogues)
        actions_bar.addWidget(scan_btn,0,0)

        queue_whole_btn = QPushButton('Queue whole release')
        queue_whole_btn.clicked.connect(self.queue_whole_release)
        actions_bar.addWidget(queue_whole_btn,0,1)

        select_tracks_btn = QPushButton('Select tracks to queue…')
        select_tracks_btn.setToolTip('Choose specific tracks to add to the download queue')
        select_tracks_btn.clicked.connect(self.review_release)
        actions_bar.addWidget(select_tracks_btn,0,2)

        self.ignore_btn = QPushButton('Ignore release')
        self.ignore_btn.setToolTip('Hide this release from missing music views')
        self.ignore_btn.clicked.connect(self.ignore_release)
        actions_bar.addWidget(self.ignore_btn,1,1)

        self.restore_btn = QPushButton('Restore release')
        self.restore_btn.setToolTip('Restore this ignored release to active missing music')
        self.restore_btn.clicked.connect(self.restore_release)
        self.restore_btn.setVisible(False)
        actions_bar.addWidget(self.restore_btn,1,1)

        link_check_btn=QPushButton('Check availability')
        link_check_btn.setToolTip('Check up to 100 new or expired release links; recent saved results are reused')
        link_check_btn.clicked.connect(self.check_release_links)
        actions_bar.addWidget(link_check_btn,1,0)

        open_tidal_btn = QPushButton('Open on web')
        open_tidal_btn.clicked.connect(self.open_release)
        actions_bar.addWidget(open_tidal_btn,1,2)

        self.cache_status = QLabel()
        self.cache_status.setObjectName('muted')
        actions_bar.addWidget(self.cache_status,0,3,2,1)
        actions_bar.setColumnStretch(3,1)

        layout.addLayout(actions_bar)

        bar = QGridLayout();bar.setHorizontalSpacing(8);bar.setVerticalSpacing(8)
        self.search = QLineEdit(); self.search.setPlaceholderText('Filter artist or release…'); bar.addWidget(self.search,0,0,1,2)
        self.filter = QComboBox(); self.filter.addItems(['All statuses', 'Missing release', 'Queued', 'Owned partial', 'Alternate edition', 'Owned complete', 'Present locally', 'Needs review', 'Unavailable', 'Ignored']); bar.addWidget(self.filter,0,2)
        self.timeline = QComboBox(); self.timeline.addItems(['Newer than newest owned', 'Between newest two owned', 'All missing releases', 'Incomplete albums', 'All dates']); bar.addWidget(self.timeline,0,3)
        self.kind = QComboBox(); self.kind.addItems(['All types', 'ALBUM', 'EP', 'SINGLE']); bar.addWidget(self.kind,1,0)
        self.copyright_filter = QComboBox(); self.copyright_filter.addItems(['All copyrights', 'Matching local copyrights', 'No copyright match']); bar.addWidget(self.copyright_filter,1,1)
        self.recommendation_filter = QComboBox(); self.recommendation_filter.addItems(['All recommendations', 'Recommended', 'Potential', 'Suspect / Low match', 'Unmatched']); bar.addWidget(self.recommendation_filter,1,2,1,2)
        bar.setColumnStretch(0,2);bar.setColumnStretch(1,2);bar.setColumnStretch(2,1);bar.setColumnStretch(3,1)
        layout.addLayout(bar)
        self.search.textChanged.connect(self.refresh_coverage); self.filter.currentTextChanged.connect(self.refresh_coverage)
        self.kind.currentTextChanged.connect(self.refresh_coverage)
        self.timeline.currentTextChanged.connect(self.refresh_coverage)
        self.copyright_filter.currentTextChanged.connect(self.refresh_coverage)
        self.recommendation_filter.currentTextChanged.connect(self.refresh_coverage)
        self.coverage_table = table(['Album Artist', 'Release', 'Date', 'Type', 'Tracks', 'Coverage', 'Quality', 'Online ID', 'Recommendation', 'Select'], virtual=True)
        self._missing_selection={}
        self._missing_selection_items={}
        self.coverage_table.model().check_column=9
        self.coverage_table.model().checkedChanged.connect(lambda key,checked:QTimer.singleShot(0,lambda:self._missing_checked(key,checked)))
        self.coverage_table.horizontalHeader().setStretchLastSection(False)
        self.coverage_table.horizontalHeader().setSectionResizeMode(9,QHeaderView.ResizeMode.Fixed)
        self.coverage_table.setColumnWidth(9,76)
        self.coverage_table.horizontalHeader().moveSection(9,0)
        selection_bar=QHBoxLayout()
        select_all=QPushButton('Select visible');select_all.clicked.connect(lambda:self._select_missing_visible(True));selection_bar.addWidget(select_all)
        clear=QPushButton('Clear selection');clear.clicked.connect(lambda:self._select_missing_visible(False));selection_bar.addWidget(clear)
        self.queue_selected_btn=QPushButton('Queue selected (0)');self.queue_selected_btn.clicked.connect(self.queue_checked_missing);selection_bar.addWidget(self.queue_selected_btn);selection_bar.addStretch();layout.addLayout(selection_bar)
        self.coverage_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection);self.coverage_table.setStyleSheet('QTableView::item:selected { background: rgba(128,128,128,45); color: palette(text); }')
        self.coverage_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.coverage_table.customContextMenuRequested.connect(self._coverage_table_context_menu)
        layout.addWidget(self.coverage_table)
        self.coverage_table.itemDoubleClicked.connect(self.toggle_release_expansion)
        self.coverage_table.itemSelectionChanged.connect(self._update_missing_actions)
        self.filter.currentTextChanged.connect(lambda _: self._update_missing_actions())
        for col, width in [(0, 240), (1, 280), (2, 110), (3, 95), (4, 80), (5, 140), (6, 130), (7, 130)]:
            self.coverage_table.setColumnWidth(col, width)
            self.coverage_table.horizontalHeader().setSectionResizeMode(col, QHeaderView.ResizeMode.Interactive)
        self.coverage_table.horizontalHeader().setMinimumSectionSize(60)
        self.coverage_table.horizontalHeader().setStretchLastSection(True)

    def queue_page(self):
        layout = self.page('Download Releases', 'Approve releases or selected tracks for download or export. Your checklist is saved automatically.')
        self.queue_page_widget=layout.parentWidget()
        # Filter bar
        queue_filter_bar = QHBoxLayout()
        self.queue_search = QLineEdit()
        self.queue_search.setPlaceholderText('Filter artist, release or track…')
        self.queue_search.textChanged.connect(self.filter_queue_table)
        queue_filter_bar.addWidget(self.queue_search, 2)

        self.queue_status_filter = QComboBox()
        self.queue_status_filter.addItems(['All items', 'Approved only', 'Unapproved only'])
        self.queue_status_filter.currentTextChanged.connect(self.filter_queue_table)
        queue_filter_bar.addWidget(self.queue_status_filter)

        self.queue_kind_filter = QComboBox()
        self.queue_kind_filter.addItems(['All types', 'ALBUM', 'EP', 'SINGLE'])
        self.queue_kind_filter.currentTextChanged.connect(self.filter_queue_table)
        queue_filter_bar.addWidget(self.queue_kind_filter)

        self.queue_start_download_btn = QPushButton('Start download')
        self.queue_start_download_btn.clicked.connect(self.start_downloads)
        queue_filter_bar.addWidget(self.queue_start_download_btn)

        self.queue_export_btn = QPushButton('Export release list…')
        self.queue_export_btn.clicked.connect(self.export_queue)
        queue_filter_bar.addWidget(self.queue_export_btn)

        layout.addLayout(queue_filter_bar)

        self.queue_table = table(['Approve', 'Album Artist', 'Release', 'Selection', 'Date', 'Type', 'Tracks', 'Availability'])
        self.queue_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection);self.queue_table.setStyleSheet('QTableWidget::item:selected { background: rgba(128,128,128,45); color: palette(text); }'); layout.addWidget(self.queue_table)
        self.queue_table.setSortingEnabled(False)
        self._queue_sort=None
        self.queue_table.horizontalHeader().setSectionsClickable(True)
        self.queue_table.horizontalHeader().sectionClicked.connect(self._sort_queue)
        self.queue_table.itemChanged.connect(self.queue_changed)
        self.queue_table.itemDoubleClicked.connect(self.toggle_queue_expansion)
        self.queue_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.queue_table.customContextMenuRequested.connect(self._queue_table_context_menu)
        self.queue_table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.Fixed)
        self.queue_table.setColumnWidth(0, 75)
        for col, width in [(1, 240), (2, 280), (3, 140), (4, 125), (5, 95), (6, 80), (7, 140)]:
            self.queue_table.setColumnWidth(col, width)
            self.queue_table.horizontalHeader().setSectionResizeMode(col, QHeaderView.ResizeMode.Interactive)
        self.queue_table.horizontalHeader().setMinimumSectionSize(60)
        self.queue_table.horizontalHeader().setStretchLastSection(True)

        self.queue_dest_note = QLabel(f"Downloads save to: {self.download_folder.text()} (Change in Settings → Downloads).")
        self.queue_dest_note.setObjectName('muted')
        layout.addWidget(self.queue_dest_note)

    def downloaded_releases_page(self):
        page = QWidget()
        layout = QVBoxLayout(page)
        layout.setContentsMargins(0, 4, 0, 0)
        layout.setSpacing(12)

        header = QVBoxLayout()
        header.setSpacing(4)
        title = QLabel('Downloaded Releases')
        title.setObjectName('title')
        header.addWidget(title)
        subtitle = QLabel('Releases downloaded from online catalogue. Mark as undownloaded to return them to missing releases.')
        subtitle.setObjectName('muted')
        header.addWidget(subtitle)
        layout.addLayout(header)

        # Filter bar
        filter_bar = QHBoxLayout()
        self.downloaded_search = QLineEdit()
        self.downloaded_search.setPlaceholderText('Filter artist or release…')
        self.downloaded_search.textChanged.connect(self._filter_downloaded_table)
        filter_bar.addWidget(self.downloaded_search, 2)

        self.downloaded_type_filter = QComboBox()
        self.downloaded_type_filter.addItems(['All types', 'ALBUM', 'EP', 'SINGLE'])
        self.downloaded_type_filter.currentTextChanged.connect(self._filter_downloaded_table)
        filter_bar.addWidget(self.downloaded_type_filter)

        self.mark_undownloaded_btn = QPushButton('Mark as Undownloaded')
        self.mark_undownloaded_btn.clicked.connect(self._mark_selected_undownloaded)
        filter_bar.addWidget(self.mark_undownloaded_btn)

        layout.addLayout(filter_bar)

        self.downloaded_table = table(['Album Artist', 'Release / track', 'Downloaded Date', 'Type', 'Tracks', 'ID'],virtual=True)
        self.expanded_downloaded_releases=set()
        self.downloaded_table.clicked.connect(self._toggle_downloaded_release)
        self.downloaded_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.downloaded_table.customContextMenuRequested.connect(self._downloaded_table_context_menu)
        self.downloaded_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.downloaded_table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.Interactive)
        self.downloaded_table.horizontalHeader().setStretchLastSection(False)
        for col,width in ((0,190),(2,130),(3,90),(4,70),(5,95)):
            self.downloaded_table.setColumnWidth(col,width)
        self.downloaded_table.horizontalHeader().setSectionResizeMode(1,QHeaderView.ResizeMode.Stretch)
        layout.addWidget(self.downloaded_table, 1)

        bottom_bar = QHBoxLayout()
        self.downloaded_count_label = QLabel('0 downloaded releases')
        self.downloaded_count_label.setObjectName('muted')
        bottom_bar.addWidget(self.downloaded_count_label)
        bottom_bar.addStretch()
        layout.addLayout(bottom_bar)

        self.downloaded_rows = []
        return page

    def _downloaded_table_context_menu(self,pos):
        row=self._select_context_row(self.downloaded_table,pos)
        if row<0:return
        model=self.downloaded_table.model();ident=str(model.keys[row]) if row<len(model.keys) else ''
        release=next((r for r in self.downloaded_rows if r['id']==ident),None)
        if not release:return
        menu=QMenu(self)
        kind=model.row_types.get(row)
        release_row=next((i for i,key in enumerate(model.keys) if str(key)==ident and model.row_types.get(i)=='release'),row)
        action=menu.addAction('Collapse tracks' if ident in self.expanded_downloaded_releases else 'Show tracks')
        action.triggered.connect(lambda:self._toggle_downloaded_release(model.index(release_row,0)))
        track_id=model.rows[row][5] if kind=='track' and row<len(model.rows) else ''
        if str(track_id).isdecimal():
            open_action=menu.addAction('Open on web')
            open_action.triggered.connect(lambda:QDesktopServices.openUrl(QUrl(f'https://tidal.com/browse/track/{track_id}')))
        else:
            open_action=menu.addAction('Open on web')
            open_action.triggered.connect(lambda:QDesktopServices.openUrl(QUrl(f'https://tidal.com/browse/album/{ident}')))
        menu.addSeparator();undo=menu.addAction('Mark release as undownloaded…');undo.triggered.connect(self._mark_selected_undownloaded)
        menu.exec(self.downloaded_table.viewport().mapToGlobal(pos))

    def refresh_downloaded_releases(self):
        if not hasattr(self, 'downloaded_table'):
            return
        rows = []
        try:
            q_rows = self.store.rows("SELECT * FROM queue WHERE decision='downloaded' ORDER BY updated DESC")
            for r in q_rows:
                payload = json.loads(r['payload']) if isinstance(r['payload'], str) else (r['payload'] or {})
                artist = payload.get('artist') or 'Unknown Artist'
                title = payload.get('title') or 'Unknown Title'
                updated_val = r['updated'] or ''
                updated_date = updated_val.split('T')[0] if 'T' in updated_val else (updated_val[:10] if updated_val else '—')
                rel_type = (payload.get('type') or 'ALBUM').upper()
                tracks = payload.get('selected_tracks') if payload.get('selected_tracks') is not None else payload.get('tracks') or []
                track_count = str(len(tracks)) if tracks else str(payload.get('track_count') or '—')
                rel_id = str(r['id'])
                rows.append({
                    'id': rel_id,
                    'artist': artist,
                    'title': title,
                    'date': updated_date,
                    'type': rel_type,
                    'tracks': track_count,
                    'payload': payload,
                })
        except Exception:
            pass
        self.downloaded_rows = rows
        self._filter_downloaded_table()

    def _filter_downloaded_table(self):
        if not hasattr(self, 'downloaded_table') or not hasattr(self, 'downloaded_rows'):
            return
        query = self.downloaded_search.text().strip().casefold() if hasattr(self, 'downloaded_search') else ''
        type_filter = self.downloaded_type_filter.currentText() if hasattr(self, 'downloaded_type_filter') else 'All types'

        filtered = []
        for r in self.downloaded_rows:
            if type_filter != 'All types' and r['type'] != type_filter:
                continue
            if query:
                if query not in r['artist'].casefold() and query not in r['title'].casefold() and query not in r['id'].casefold():
                    continue
            filtered.append(r)

        display=[];keys=[];kinds={}
        for r in filtered:
            expanded=r['id'] in self.expanded_downloaded_releases
            kinds[len(display)]='release';keys.append(r['id'])
            display.append((('▾ ' if expanded else '▸ ')+r['artist'],r['title'],r['date'],r['type'],r['tracks'],r['id']))
            if expanded:
                payload=r['payload']
                tracks=payload.get('selected_tracks') if payload.get('selected_tracks') is not None else payload.get('tracks',[])
                for track in tracks:
                    kinds[len(display)]='track';keys.append(r['id'])
                    display.append(('',track.get('title','Unknown track'),'', 'Track',str(track.get('track_number','')),str(track.get('id',''))))
                if not tracks:
                    kinds[len(display)]='track';keys.append(r['id'])
                    display.append(('','Track details are not saved locally. Click the release again to retry.','','','',''))
        fill(self.downloaded_table,display,keys=keys,row_types=kinds)
        self.downloaded_count_label.setText(f'{len(filtered)} downloaded releases ({len(self.downloaded_rows)} total)')

    def _toggle_downloaded_release(self,index):
        model=self.downloaded_table.model()
        if model.row_types.get(index.row())=='track':return
        ident=str(model.keys[index.row()])
        if ident in self.expanded_downloaded_releases:
            self.expanded_downloaded_releases.remove(ident);self._filter_downloaded_table();return
        self.expanded_downloaded_releases.add(ident)
        row=next((r for r in self.downloaded_rows if r['id']==ident),None)
        if not row:return
        payload=row['payload']
        if not payload.get('tracks') and payload.get('selected_tracks') is None:
            cached=self.store.preferences(f'tag-review:{self.market}:{ident}')
            if cached.get('tracks'):payload['tracks']=cached['tracks']
            if not payload.get('tracks'):
                local=[t for t in self.store.tracks() if str(t.get('tidal_album_id',''))==ident]
                if local and len(local)==payload.get('track_count'):
                    payload['tracks']=[dict(id=t.get('tidal_track_id',''),title=t.get('title',''),track_number=t.get('track',''),path=t['path']) for t in local]
            if not payload.get('tracks') and not self.worker and not self.demo_mode:
                def work(cancel,progress):return self.api(cancel,progress).release_details(payload)
                def done(release):
                    with self.store.connect() as db:
                        current=db.execute("SELECT payload FROM queue WHERE id=? AND decision='downloaded'",(ident,)).fetchone()
                        if current:
                            saved=json.loads(current[0]);saved['tracks']=release.get('tracks',[])
                            db.execute('UPDATE queue SET payload=? WHERE id=?',(json.dumps(saved),ident))
                    self.refresh_downloaded_releases()
                self.job(work,done,label='Load downloaded release tracks')
        self._filter_downloaded_table()

    def _mark_selected_undownloaded(self):
        if not hasattr(self, 'downloaded_table'):
            return
        selected_indexes = self.downloaded_table.selectionModel().selectedRows()
        if not selected_indexes:
            QMessageBox.information(self, 'No selection', 'Select one or more downloaded releases to mark as undownloaded.')
            return
        ids_to_revert = []
        for idx in selected_indexes:
            item = self.downloaded_table.item(idx.row(), 0)
            if item:
                rel_id = item.data(Qt.ItemDataRole.UserRole) or self.downloaded_table.item(idx.row(), 5).text()
                if rel_id:
                    ids_to_revert.append(str(rel_id))
        ids_to_revert=list(dict.fromkeys(ids_to_revert))
        if not ids_to_revert:
            return
        msg = f"Mark {len(ids_to_revert)} release(s) as undownloaded?\n\nThey will be removed from Downloaded Releases and returned to Missing Releases."
        if QMessageBox.question(self, 'Mark as undownloaded', msg) != QMessageBox.StandardButton.Yes:
            return
        try:
            with self.store.connect() as db:
                db.executemany("DELETE FROM queue WHERE id=?", [(i,) for i in ids_to_revert])
            self.refresh_downloaded_releases()
            self.refresh()
            self.log(f"Marked {len(ids_to_revert)} release(s) as undownloaded · returned to missing releases")
        except Exception as exc:
            QMessageBox.warning(self, 'Database error', f"Could not mark releases as undownloaded: {exc}")

    def filter_queue_table(self):
        if not hasattr(self, 'queue_table') or not hasattr(self, 'displayed_queue_rows'):
            return
        query = self.queue_search.text().strip().casefold() if hasattr(self, 'queue_search') else ''
        status_filter = self.queue_status_filter.currentText() if hasattr(self, 'queue_status_filter') else 'All items'
        kind_filter = self.queue_kind_filter.currentText() if hasattr(self, 'queue_kind_filter') else 'All types'

        parent_visibility = {}
        for item in self.displayed_queue_rows:
            if item.get('type') == 'release':
                rel_id = item['id']
                payload = item.get('payload') or {}
                r = item.get('record') or {}
                is_approved = bool(r.get('approved'))
                rel_type = (payload.get('type') or '').upper()

                if status_filter == 'Approved only' and not is_approved:
                    parent_visibility[rel_id] = False
                    continue
                if status_filter == 'Unapproved only' and is_approved:
                    parent_visibility[rel_id] = False
                    continue

                if kind_filter != 'All types' and rel_type != kind_filter:
                    parent_visibility[rel_id] = False
                    continue

                if query:
                    artist = str(payload.get('artist') or '').casefold()
                    title = str(payload.get('title') or '').casefold()
                    if query in artist or query in title:
                        parent_visibility[rel_id] = True
                    else:
                        tracks = payload.get('tracks') or []
                        matched_track = any(query in str(t.get('title') or '').casefold() for t in tracks)
                        parent_visibility[rel_id] = matched_track
                else:
                    parent_visibility[rel_id] = True

        for i, item in enumerate(self.displayed_queue_rows):
            if item.get('type') == 'release':
                rel_id = item['id']
                hidden = not parent_visibility.get(rel_id, True)
            else:
                parent_id = item.get('parent_id')
                if not parent_visibility.get(parent_id, True):
                    hidden = True
                else:
                    if query:
                        tr = item.get('track') or {}
                        tr_title = str(tr.get('title') or '').casefold()
                        payload = {}
                        if isinstance(item.get('record'), dict) and 'payload' in item['record']:
                            try:
                                payload = json.loads(item['record']['payload']) if isinstance(item['record']['payload'], str) else item['record']['payload']
                            except Exception:
                                payload = {}
                        p_artist = str(payload.get('artist') or '').casefold()
                        p_title = str(payload.get('title') or '').casefold()
                        if query in p_artist or query in p_title or query in tr_title:
                            hidden = False
                        else:
                            hidden = True
                    else:
                        hidden = False
            self.queue_table.setRowHidden(i, hidden)

    def queue_visible_missing(self):
        if self.worker:return
        existing={r['id'] for r in self.store.rows('SELECT id FROM queue')}
        rows=[r['release'] for r in getattr(self, '_base_coverage_rows', self.coverage_rows) if r.get('state')=='Missing release' and str(r.get('release',{}).get('id')) not in existing]
        if not rows:return
        for release in rows:self.store.enqueue(release)
        self.refresh();self.log(f'{len(rows)} visible missing releases queued for approval.')

    def approve_selected_queue(self):
        if self.worker:return
        for index in self.queue_table.selectionModel().selectedRows():
            ident=self.queue_table.item(index.row(),0).data(Qt.ItemDataRole.UserRole)
            if ident is not None:self.set_queue_approval(str(ident),True)
        self.refresh_queue()

    def choose_download_folder(self):
        if self.worker:return
        folder=QFileDialog.getExistingDirectory(self,'Download folder',self.download_folder.text())
        if folder:
            self.download_folder.setText(folder);self.settings.setValue('download_folder',folder)
            if hasattr(self, 'queue_dest_note'):
                self.queue_dest_note.setText(f"Downloads save to: {folder} (Change in Settings → Downloads).")

    def download_auth_dialog(self, url, replies):
        self.open_authorization(url)
        dialog=QInputDialog(self);dialog.setWindowTitle('Connect subscriber download account')
        dialog.setLabelText('Sign in in your browser. Online may finish on an “Oops” page. Paste the complete final browser URL here. It is used only for sign-in and is not logged.')
        dialog.setTextEchoMode(QLineEdit.EchoMode.Password);dialog.resize(640,220)
        timer=QTimer(dialog);timer.setSingleShot(True);timer.timeout.connect(dialog.reject);timer.start(180000)
        replies.put(dialog.textValue().strip() if dialog.exec() else '')

    def start_downloads(self, checked=False, connect_only=False):
        if self.worker:return
        if self.demo_mode:return self.demo_notice()
        approved=self.store.rows("SELECT id FROM queue WHERE approved=1 AND decision='queued'")
        if not approved and not connect_only:
            QMessageBox.information(self,'Nothing approved','Check the approval boxes for the releases or track selections you want to download.');return
        output=self.download_folder.text()
        if not connect_only and QMessageBox.question(self,'Download approved music',f'Download {len(approved)} approved queue items to:\n{output}\n\nOnly the selected tracks are included for partial releases. Existing files are not overwritten.')!=QMessageBox.StandardButton.Yes:return
        def authenticate(url,cancel):
            replies=queue.Queue();self.download_auth_requested.emit(url,replies)
            deadline=time.monotonic()+185
            while not cancel() and time.monotonic()<deadline:
                try:return replies.get(timeout=.2)
                except queue.Empty:pass
            return ''
        self.details_button.setChecked(True)
        self.queue_page_widget.setEnabled(False)
        self.job(lambda cancel,progress:download_approved(self.store,output,cancel,progress,authenticate,connect_only=connect_only),label='Connect download account' if connect_only else 'Download approved music', is_disk_op=(not connect_only))

    def update_category_cards(self,data):
        cards=getattr(self,'category_cards',{})
        self._update_card_favs_metric()
        stats=data.get('link_statistics',{})
        mappings=data.get('mappings',{})
        artists=data.get('artists',{})
        linked_artists=sum(mappings.get(name,{}).get('status') in ('confirmed','auto') for name in artists)
        queue=data.get('queue',[])
        metrics={('Link Catalogue',0):f"{linked_artists:,} / {len(artists):,} artists linked",
                 ('Link Catalogue',1):f"{stats.get('linked_releases',0):,} / {stats.get('release_count',0):,} releases linked",
                 ('Link Catalogue',2):self.card_favs_metric.text(),
                 ('Complete Library',0):'Review available releases',
                 ('Complete Library',1):f"{sum(r['decision']=='queued' for r in queue):,} releases queued",
                 ('Complete Library',2):f"{sum(r['decision']=='downloaded' for r in queue):,} completed",
                 ('Settings',0):f"{len(data.get('roots',[]))} libraries",('Settings',1):('Not checked' if not hasattr(self,'_connection_metrics') else 'All connected' if len(self._connection_metrics)>=4 and all(v.get('ok') for v in self._connection_metrics.values()) else 'Needs attention'),
                 ('Settings',2):self.download_quality.currentText()}
        for key,metric in metrics.items():
            if key in cards:
                card=cards[key];card.setText(card.text().split('\n')[0]+'\n'+metric)
                if key==('Settings',0):
                    counts=data.get('library_link_counts',[])
                    card.description_label.setText('\n'.join(f"{Path(r['root']).name}: {r['linked_tracks']:,} / {r['track_count']:,} tracks linked" for r in counts) or 'No libraries added')
                    card.setToolTip('\n'.join(f"{r['root']}: {r['linked_tracks']:,} / {r['track_count']:,} tracks linked" for r in counts))
                elif key==('Settings',2):card.description_label.setText(f"{self.provider_settings['download_concurrency']} parallel downloads · {self.download_folder.text()}")
                elif key==('Settings',1) and hasattr(self,'_connection_metrics'):
                    names={'search':'Catalogue','account':'Account','credentials':'App credentials','download':'Downloads','check':'Connection check'}
                    failed=[names.get(k,k) for k,v in self._connection_metrics.items() if not v.get('ok')]
                    card.description_label.setText('Check: '+', '.join(failed) if failed else 'Catalogue, account and downloads are ready')

    def category_pages(self):
        self.category_cards={}
        specs=[('Link Catalogue','Connect local artists and releases to verified online matches.',[
            ('👤  Link Artists',1,'Review artist identities'),('🔗  Link Releases',2,'Match recordings and editions'),('⭐  Favourite Artists',7,'Compare saved favourites')]),
            ('Complete Library','Review missing music, approve downloads and track completed releases.',[
            ('🔍  Missing Releases',3,'Discover releases and missing tracks'),('📥  Download Queue',4,'Approve exactly what to download'),('✅  Downloaded Releases',9,'Review completed downloads')]),
            ('Settings','Configure your library, streaming connections and downloads.',[
            ('⚙️  General',6,'Appearance, libraries and matching'),('🔑  Connections',6,'Credentials and metadata connections'),('⬇️  Downloads',6,'Quality, destination and download engine')])]
        for title,description,items in specs:
            layout=self.page(title,description);grid=QGridLayout();grid.setSpacing(12);layout.addLayout(grid)
            for n,(name,target,detail) in enumerate(items):
                card=HealthCard(name,detail);card.setText(name+'\nReady')
                def navigate(target=target,tab=n,settings=title=='Settings'):
                    self.nav.setCurrentRow(target)
                    if settings:self.settings_tabs.setCurrentIndex(tab)
                card.activated.connect(navigate);grid.addWidget(card,n//2,n%2)
                self.category_cards[(title,n)]=card
            layout.addStretch()

    def remediation_page(self):
        layout=self.page('Fix Library','Use verified online links to review missing metadata, artwork and replacement releases.')
        grid=QGridLayout();grid.setSpacing(12);layout.addLayout(grid)
        for col,key in enumerate(('metadata','artwork')):grid.addWidget(self.tools_summary_buttons[key],0,col)
        self.remote_health_card=HealthCard('♻️  Online Replacements','Find larger available releases containing your existing recordings')
        self.remote_health_card.setText('♻️  Online Replacements\nInspect first')
        self.remote_health_card.activated.connect(lambda:self.nav.setCurrentRow(12));grid.addWidget(self.remote_health_card,1,0)
        layout.addStretch()

    def tools_page(self):
        layout = self.page('Prepare Library', 'Check your library, then work on one kind of change at a time.')
        self.tools_page_widget=layout.parentWidget();self.tools_plan=[];self.tools_snapshots={}
        self.tools_root=QComboBox();self.tools_root.hide()
        self.tools_inspection_status=QLabel();self.tools_inspection_status.hide()
        self.tools_tabs=WorkflowTabs();self.tools_tabs.setDocumentMode(False);self.tools_tabs.tabBar().setExpanding(False)
        self.tools_tabs.tabBar().setElideMode(Qt.TextElideMode.ElideNone)
        self.tools_tabs.tabBar().hide()
        self.tools_tabs.setSizePolicy(QSizePolicy.Policy.Expanding,QSizePolicy.Policy.Maximum);layout.addWidget(self.tools_tabs)
        self.tools_summary_buttons={}
        descriptions=[
            ('Summary','Prepare Library','Work on any operation at any time without having to step through a full wizard. Use Fix existing tags for offline cleanup, Add missing tags to enrich metadata from online source, Fix artwork for high-resolution covers, or Organise files for file and folder maintenance.'),
            ('Fix existing tags','Standardise audio tags · offline','Standardise release dates, disc numbers, and Camelot keys (INITIALKEY), or remove lyrics tags. Offline only; files stay in place and audio tags change only when you review and click Update tags.'),
            ('Add missing tags','Apply online metadata from online source','Enrich your local files with missing BPM, musical keys, release dates, and catalogue tags from your verified Online links. Existing valid tags are preserved and files stay in place.'),
            ('Fix artwork','Standardise front covers','Find genuine 1280 × 1280 front covers, or downsize larger square artwork. Smaller images are never upscaled. Tags and file locations stay unchanged.'),
            ('Organise files','Manage file locations and redundant singles · offline','Arrange folders and filenames from saved tags, move redundant singles to Trash, and reunite stray tracks into parent album folders. Audio tags remain unchanged.')]
        for index,(title,heading,description) in enumerate(descriptions):
            page=QWidget();body=QVBoxLayout(page);body.setContentsMargins(10,8,10,8);body.setSpacing(6)
            body.setAlignment(Qt.AlignmentFlag.AlignTop)
            label=QLabel(heading);font=label.font();font.setBold(True);label.setFont(font)
            if index != 0:
                body.addWidget(label)
                label=QLabel(description);label.setWordWrap(True);body.addWidget(label)
            if index==0:
                body.setContentsMargins(10, 8, 10, 8)
                grid=QGridLayout();grid.setSpacing(12);grid.setContentsMargins(0, 4, 0, 4)
                body.addLayout(grid)
                labels = {
                    'tags': 'Correct tags',
                    'organise': 'Organise files',
                    'metadata': 'Add missing tags',
                    'artwork': 'Fix artwork'
                }
                grid.addWidget(QLabel('Local preparation'),0,0,1,2)
                grid.addWidget(QLabel('Local integrity'),2,0,1,2)
                for i, (key, label) in enumerate(labels.items()):
                    icons={'tags':'🏷️','organise':'📁','metadata':'➕','artwork':'🖼️'}
                    card_descriptions={'tags':'Dates, track/disc numbers, musical keys and lyrics', 'organise':'Folder layout and redundant files',
                                  'metadata':'Missing tags from verified online links','artwork':'Front covers below the target size'}
                    button = HealthCard(f'{icons[key]}  {label}',card_descriptions[key])
                    button.activated.connect(lambda n={'tags':1,'metadata':2,'artwork':3,'organise':4}[key]: self.tools_tabs.setCurrentIndex(n))
                    if key in ('tags','organise'):grid.addWidget(button,1,i%2)
                    self.tools_summary_buttons[key] = button
                self.health_audit_cards={}
                for col,(key,title,detail,page_index) in enumerate((('mqa','🔬  MQA Audit','Inspect encoding signals; files stay unchanged',10),('local','♻️  Local Consolidation','Review duplicate recordings in complete local albums',11))):
                    card=HealthCard(title,detail);card.setText(title+'\nInspect first')
                    card.activated.connect(lambda n=page_index:self.nav.setCurrentRow(n));grid.addWidget(card,3,col);self.health_audit_cards[key]=card
                self.tools_summary_status=QLabel();self.tools_summary_status.setWordWrap(True);body.addWidget(self.tools_summary_status)
                refresh_health=QPushButton('Refresh local tags');refresh_health.clicked.connect(self.inspect_library)
                body.addWidget(refresh_health,alignment=Qt.AlignmentFlag.AlignLeft)
            elif index==1:
                action_box = QVBoxLayout(); action_box.setSpacing(10)
                action_label = QLabel('Choose one tag correction to preview:')
                action_label.setObjectName('muted'); action_box.addWidget(action_label)

                grid = QGridLayout()
                grid.setHorizontalSpacing(32)
                grid.setVerticalSpacing(10)
                grid.setContentsMargins(4, 2, 4, 6)

                self.tools_dates = OperationCard('Clean date formatting','Use YYYY, YYYY-MM, or YYYY-MM-DD consistently. Files stay in place.')
                self.tools_discs = OperationCard('Standardise track and disc numbers','Pad track/disc numbers to 01 and repair totals from consistent local tags or verified cached matches. Files stay in place.')
                self.tools_keys = OperationCard('Save musical keys as Camelot (INITIALKEY)','Convert existing usable musical keys to Camelot notation in INITIALKEY.')
                self.tools_remove_lyrics = OperationCard('Remove lyrics tags','Remove embedded lyrics only. Audio and every other tag are preserved.')

                self.tools_dates.setChecked(False)
                self.tools_discs.setChecked(False)
                self.tools_keys.setChecked(False)
                self.tools_remove_lyrics.setChecked(False)

                grid.addWidget(self.tools_dates, 0, 0)
                grid.addWidget(self.tools_discs, 1, 0)
                grid.addWidget(self.tools_keys, 0, 1)
                grid.addWidget(self.tools_remove_lyrics, 1, 1)

                action_box.addLayout(grid)
                body.addLayout(action_box)
            elif index==2:
                button=QPushButton('Find missing tags on online source')
                button.setToolTip('Queries Online for missing tags on selected files (or all visible files if none are selected) and stages them in the table for review before applying.')
                button.clicked.connect(self.fill_library_tags)
                body.addWidget(button,alignment=Qt.AlignmentFlag.AlignLeft)
                helper=QLabel('Queries Online for missing tags (BPM, musical keys, release dates) on selected or visible files, staging proposed changes in the table for your review before applying.')
                helper.setWordWrap(True);helper.setObjectName('muted');body.addWidget(helper)
            elif index==3:
                button=QPushButton('Find 1280 × 1280 covers')
                button.clicked.connect(self.fill_library_tags)
                body.addWidget(button,alignment=Qt.AlignmentFlag.AlignLeft)
            elif index==4:
                action_box = QVBoxLayout(); action_box.setSpacing(10)
                action_label = QLabel('Choose one file or folder action to preview:')
                action_label.setObjectName('muted'); action_box.addWidget(action_label)

                org_grid = QGridLayout()
                org_grid.setHorizontalSpacing(32)
                org_grid.setVerticalSpacing(10)
                org_grid.setContentsMargins(4, 2, 4, 6)

                self.tools_org_layout = OperationCard('Arrange folders and filenames','Move or rename files from their saved tags and your selected layout template.')
                self.tools_org_superseded = OperationCard('Remove superseded singles','Move redundant standalone singles to macOS Trash after review.')
                self.tools_org_strays = OperationCard('Reunite stray tracks','Move verified stray tracks into their wider local album folder.')
                self.tools_org_duplicates = OperationCard('Find duplicate track positions','Show folders containing more than one file in the same disc and track position.')

                self.tools_org_layout.setChecked(False)
                self.tools_org_superseded.setChecked(False)
                self.tools_org_strays.setChecked(False)
                self.tools_org_duplicates.setChecked(False)

                # Aliases for backwards compatibility
                self.tools_organise_local = self.tools_org_layout
                self.tools_stray = self.tools_org_strays
                self.tools_duplicate_tracks = self.tools_org_duplicates

                org_grid.addWidget(self.tools_org_layout, 0, 0)
                org_grid.addWidget(self.tools_org_superseded, 1, 0)
                org_grid.addWidget(self.tools_org_strays, 0, 1)
                org_grid.addWidget(self.tools_org_duplicates, 1, 1)

                action_box.addLayout(org_grid)
                body.addLayout(action_box)
            self.tools_tabs.addTab(page,title if index==0 else descriptions[index][0])
        from PySide6.QtWidgets import QButtonGroup
        self.tag_action_group = QButtonGroup(self)
        self.organise_action_group = QButtonGroup(self)
        for option in (self.tools_dates, self.tools_discs, self.tools_keys, self.tools_remove_lyrics):
            self.tag_action_group.addButton(option)
        for option in (self.tools_org_layout, self.tools_org_superseded, self.tools_org_strays, self.tools_org_duplicates):
            self.organise_action_group.addButton(option)
        self.tools_workspace=QWidget();self.tools_workspace.setSizePolicy(QSizePolicy.Policy.Expanding,QSizePolicy.Policy.Expanding)
        work=QVBoxLayout(self.tools_workspace);work.setContentsMargins(0,0,0,0);work.setSpacing(10);layout.addWidget(self.tools_workspace,1)
        bar=QHBoxLayout();self.tools_search=QLineEdit();self.tools_search.setPlaceholderText('Find a file, artist or album…');bar.addWidget(self.tools_search,1)
        self.tools_filter=QComboBox();bar.addWidget(self.tools_filter)
        inspect_tags_btn = QPushButton('Refresh local tags')
        inspect_tags_btn.clicked.connect(self.inspect_library)
        bar.addWidget(inspect_tags_btn)
        self.update_filter_options()
        self.tools_scope=QComboBox();self.tools_scope.addItems(['All visible files','Selected files']);self.tools_scope.setToolTip('The scope used for Online checks, guided review and manual tag edits.');bar.addWidget(self.tools_scope);self.tools_scope.hide();work.addLayout(bar)
        self.tools_dj_only=QCheckBox('Only files missing BPM or musical key');self.tools_dj_only.hide()
        self.tools_scope_status=QLabel();work.addWidget(self.tools_scope_status)
        self.tools_table=table(['File','Needs attention','Proposed changes','Destination','Result'],virtual=True);self.tools_table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.tools_table.setSizePolicy(QSizePolicy.Policy.Expanding,QSizePolicy.Policy.Expanding);self.tools_table.setMinimumHeight(280);work.addWidget(self.tools_table,1)
        self.tools_table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.tools_table.customContextMenuRequested.connect(self._tools_table_context_menu)
        self.tools_detail=QPlainTextEdit();self.tools_detail.setReadOnly(True);self.tools_detail.setMinimumHeight(100);self.tools_detail.setMaximumHeight(155);self.tools_detail.setPlaceholderText('Select a file to see its current album artist, track artist and exact proposed changes.');self.tools_detail.hide();work.addWidget(self.tools_detail)
        self.tools_status=QLabel();self.tools_status.setWordWrap(True);work.addWidget(self.tools_status)
        footer=QHBoxLayout()
        select=QPushButton('Select visible changes');select.clicked.connect(self.tools_select_changes);footer.addWidget(select)
        inspect=QPushButton('Inspect all tags…');inspect.clicked.connect(self.tools_inspect_tags);footer.addWidget(inspect)
        self.tools_details_btn=QPushButton('Show details');self.tools_details_btn.setCheckable(True)
        self.tools_details_btn.toggled.connect(lambda visible:(self.tools_detail.setVisible(visible),self.tools_details_btn.setText('Hide details' if visible else 'Show details')))
        footer.addWidget(self.tools_details_btn)
        footer.addStretch()
        self.tools_apply_button=QPushButton('Apply…');self.tools_apply_button.clicked.connect(self.tools_apply);footer.addWidget(self.tools_apply_button);work.addLayout(footer)
        self.tools_summary_space=QWidget();self.tools_summary_space.setSizePolicy(QSizePolicy.Policy.Preferred,QSizePolicy.Policy.Expanding);layout.addWidget(self.tools_summary_space,1)
        self.tools_root.currentIndexChanged.connect(self._on_tools_root_changed)
        self.tools_tabs.currentChanged.connect(self._on_tools_tab_changed)
        self.tools_search.textChanged.connect(self.filter_tools);self.tools_filter.currentTextChanged.connect(self.filter_tools);self.tools_dj_only.toggled.connect(self.filter_tools)
        self.tools_scope.currentIndexChanged.connect(self.update_tools_selection)
        self.tools_table.itemSelectionChanged.connect(self.update_tools_selection)
        for option in (self.tools_dates,self.tools_discs,self.tools_remove_lyrics,self.tools_keys,
                       self.tools_org_layout,self.tools_org_superseded,self.tools_org_strays,self.tools_org_duplicates):
            option.toggled.connect(self.invalidate_tools_plan)
        self.invalidate_tools_plan();self.tools_tabs.fit_current_page()

    def tools_operation(self):
        from .library_workflows import OPERATIONS
        idx = self.tools_tabs.currentIndex()
        if idx < len(OPERATIONS):
            return OPERATIONS[idx]
        return 'tags'

    def update_filter_options(self):
        if not hasattr(self, 'tools_filter') or not hasattr(self, 'tools_tabs'): return
        mode = self.tools_operation()
        prev = self.tools_filter.currentText()
        self.tools_filter.blockSignals(True)
        self.tools_filter.clear()
        if mode == 'organise':
            items = ['All files', 'Needs attention', 'Proposed changes', 'Folder moves only', 'Superseded singles', 'Duplicate track numbers', 'Stray tracks (mixed artist)', 'Ignored files']
        elif mode == 'tags':
            items = ['All files', 'Needs attention', 'Proposed changes', 'Unstandardised dates', 'Disc numbers', 'Ignored files']
        elif mode == 'metadata':
            items = ['All files', 'Needs attention', 'Proposed changes', 'Missing BPM', 'Missing musical key', 'Ignored files']
        elif mode == 'artwork':
            items = ['All files', 'Needs attention', 'Proposed changes', 'Ignored files']
        elif mode == 'links':
            items = ['All files', 'Needs attention', 'Unlinked tracks', 'Linked tracks', 'Too many editions / Needs choice', 'Ignored files']
        else:
            items = ['All files', 'Needs attention', 'Proposed changes', 'Ignored files']
        self.tools_filter.addItems(items)
        idx = self.tools_filter.findText(prev)
        if idx < 0:
            if prev in ('Needs cleanup / fixes', 'Needs attention'):
                idx = self.tools_filter.findText('Needs attention')
            elif prev in ('Proposed tag changes', 'Proposed artwork changes', 'Proposed changes'):
                idx = self.tools_filter.findText('Proposed changes')
        self.tools_filter.setCurrentIndex(idx if idx >= 0 else 0)
        self.tools_filter.blockSignals(False)

    def _on_tools_tab_changed(self, index=None):
        self.tools_workspace.setVisible(self.tools_operation()!='summary')
        self.tools_summary_space.setVisible(self.tools_operation()=='summary')
        self.tools_tabs.fit_current_page()
        self.update_filter_options()
        self.tools_filter.setCurrentText('Proposed changes' if self.tools_operation() in ('tags','organise') else 'Needs attention')
        self.invalidate_tools_plan()

    def update_action_counts(self, snapshot):
        if not snapshot or not hasattr(self, 'tools_dates'): return
        from .maintenance import date_changes, disc_changes, track_changes, stray_changes, detect_superseded_singles
        from .musical_keys import key_changes
        from .library_workflows import LYRIC_TAGS
        ignored_paths = self.store.ignored_local_files()
        from .number_repairs import number_repairs
        numeric=number_repairs(snapshot)
        d_count = disc_count = key_count = lyr_count = stray_count = org_count = 0
        strays = stray_changes(snapshot)
        superseded = detect_superseded_singles(snapshot)
        sup_count = sum(1 for p in superseded if p not in ignored_paths)
        for row in snapshot:
            p = row.get('path')
            if p in ignored_paths:
                continue
            tags = row.get('tags', {})
            if date_changes(tags)[0]: d_count += 1
            if numeric.get(p,({},[]))[0]: disc_count += 1
            if key_changes(tags)[0]: key_count += 1
            if any(k in tags for k in LYRIC_TAGS): lyr_count += 1
            if p in strays: stray_count += 1
            target = row.get('layout_target')
            if not target and row.get('root') and 'tags' in row:
                try:
                    from .maintenance import destination
                    target = str(destination(Path(row['root']), row.get('tags', {}), row.get('multi_disc', False), Path(row['path']), row.get('layout')))
                except Exception:
                    target = row.get('path')
            if (target and target != row.get('path')) or row.get('target', row.get('path')) != row.get('path'):
                org_count += 1
        self.tools_dates.set_count(d_count)
        self.tools_discs.set_count(disc_count)
        self.tools_keys.set_count(key_count)
        self.tools_remove_lyrics.set_count(lyr_count)
        if hasattr(self, 'tools_org_layout'):
            self.tools_org_layout.set_count(org_count)
        if hasattr(self, 'tools_org_superseded'):
            self.tools_org_superseded.set_count(sup_count)
        if hasattr(self, 'tools_org_strays'):
            self.tools_org_strays.set_count(stray_count)
        if hasattr(self, 'tools_org_duplicates'):
            dup_slots = set()
            seen_slots = set()
            for row in snapshot:
                if row.get('path') in ignored_paths:
                    continue
                tags = row.get('tags', {})
                slot = (str(Path(row.get('path', '')).parent), tags.get('tracknumber', [''])[0])
                if slot[1]:
                    if slot in seen_slots: dup_slots.add(slot)
                    else: seen_slots.add(slot)
            dup_count = sum(1 for row in snapshot if row.get('path') not in ignored_paths and (str(Path(row.get('path', '')).parent), row.get('tags', {}).get('tracknumber', [''])[0]) in dup_slots)
            self.tools_org_duplicates.set_count(dup_count)
        if hasattr(self, 'card_hygiene_metric'):
            needs_fix_files = set()
            unignored_count = 0
            for row in snapshot:
                p = row.get('path')
                if p in ignored_paths:
                    continue
                unignored_count += 1
                tags = row.get('tags', {})
                if (date_changes(tags)[0] or disc_changes(tags)[0] or track_changes(tags)[0] or key_changes(tags)[0] or any(k in tags for k in LYRIC_TAGS)):
                    needs_fix_files.add(p)
            count = unignored_count
            fix_pct = round(100 * len(needs_fix_files) / count) if count else 0
            if not count:
                self.card_hygiene_metric.setText('—')
                self.card_hygiene_sub.setText('No local files scanned')
            elif needs_fix_files:
                self.card_hygiene_metric.setText(f'{fix_pct}% require fixes')
                self.card_hygiene_sub.setText(f'{len(needs_fix_files):,} of {count:,} local files need tag cleanups')
            else:
                self.card_hygiene_metric.setText('100% clean')
                self.card_hygiene_sub.setText('All dates, discs and keys standardised')

    def invalidate_layout_preview(self):
        self._preview_cache={};self._preview_input_signature=None
        self._prepared_mode=None;self._health_counts_signature=None

    def invalidate_tools_plan(self, *args):
        if self._closing_requested:return
        if hasattr(self,'tools_filter') and self.tools_operation() in ('tags','organise'):
            self.tools_filter.setCurrentText('Proposed changes')
        if self.tools_plan:self.tools_snapshots[self.tools_plan[0]['root']]=self.tools_plan
        root=self.tools_root.currentData()
        if not root:
            self.render_tools_plan();return
        snapshot=self.tools_snapshots.get(root)
        if not snapshot:
            snapshot = self._load_cached_snapshot(root)
            self.tools_snapshots[root] = snapshot
        layout=self.store.preferences('organisation')
        org_layout = getattr(self, 'tools_org_layout', None) and self.tools_org_layout.isChecked()
        org_superseded = getattr(self, 'tools_org_superseded', None) and self.tools_org_superseded.isChecked()
        org_strays = getattr(self, 'tools_org_strays', None) and self.tools_org_strays.isChecked()
        org_duplicates = getattr(self, 'tools_org_duplicates', None) and self.tools_org_duplicates.isChecked()
        options = (
            self.tools_operation(),
            self.tools_dates.isChecked(),
            self.tools_discs.isChecked(),
            self.tools_remove_lyrics.isChecked(),
            self.tools_keys.isChecked(),
            org_layout,
            org_superseded,
            org_strays,
            org_duplicates
        )
        signature=(root,str(layout),tuple((r['path'],tuple(r.get('stamp',())),
                   str(r.get('metadata_changes',{})),str(r.get('artwork_proposal',{})),
                   str(r.get('overrides',{})),str(r.get('linked_ids',{}))) for r in snapshot))
        if getattr(self,'_preview_input_signature',None)!=signature or root in self._dirty_roots:
            self._preview_cache={};self._preview_input_signature=signature
        cached=getattr(self,'_preview_cache',{}).get(options)
        if cached is not None and not self._preview_worker:
            self._preview_pending=None;self._preview_ready((root,options,cached,{}));return
        self._preview_pending=(root,snapshot,layout,options)
        self.tools_apply_button.setEnabled(False)
        if self._preview_worker and self._preview_worker.isRunning():
            return
        if self._preview_worker:return
        self._start_preview()

    def _start_preview(self):
        from .library_workflows import workflow_plan
        root,snapshot,layout,options=self._preview_pending;self._preview_pending=None
        snapshot=self.tools_snapshots.get(root,snapshot)
        generation=self._scan_generations.get(root,0)
        mode,dates,discs,remove_lyrics,keys,org_layout,org_superseded,org_strays,org_duplicates=options
        refresh_files = bool(root and mode != 'summary' and (root in self._dirty_roots or (not snapshot and root not in self._prepared_roots)))
        if root and not Path(root).is_dir():
            refresh_files = False
        self._preview_error=False
        self.tools_status.setText('Preparing the current preview…')
        def prepare(cancel=lambda:False,progress=lambda _:None):
            source=[dict(row,layout=layout) for row in snapshot]
            if refresh_files:
                from .freshness import prepare_library
                source=prepare_library(self.store,root,source,cancel,progress)

            if mode in ('links','metadata','artwork'):
                from .linking import attach_links
                attach_links(source,self.store,self.market)
            plan=source if mode=='summary' else workflow_plan(source,mode,dates=dates,discs=discs,remove_lyrics=remove_lyrics,keys=keys,
                               arrange_layout=org_layout,superseded_singles=org_superseded,stray=org_strays,duplicate_tracks=org_duplicates)
            return root,options,plan,{'generation':generation}
        self.tools_workspace.setEnabled(False)
        self._preview_worker=Worker(prepare)
        self._preview_worker.message.connect(self.log,Qt.ConnectionType.QueuedConnection)
        self._preview_worker.result.connect(self._preview_ready,Qt.ConnectionType.QueuedConnection)
        self._preview_worker.failure.connect(self._preview_failed,Qt.ConnectionType.QueuedConnection)
        self._preview_worker.finished.connect(self._preview_finished,Qt.ConnectionType.QueuedConnection)
        self.scan_activity.setText('Preparing library…');self.scan_activity.show()
        self._preview_worker.message.connect(self.scan_activity.setText,Qt.ConnectionType.QueuedConnection)
        self._preview_worker.start()

    @Slot(object)
    def _preview_ready(self,result):
        if self._closing_requested:return
        root,options,plan,counts=result
        if self._preview_worker and self._preview_worker.isInterruptionRequested():return
        if counts.get('generation',self._scan_generations.get(root,0))==self._scan_generations.get(root,0):
            self.tools_snapshots[root]=plan
            self._dirty_roots.discard(root);self._prepared_roots.add(root)
        else:return
        if self._preview_pending:return
        if root!=self.tools_root.currentData() or options[0]!=self.tools_operation():return
        self._prepared_mode=(root,options[0])
        from .library_workflows import attention
        signature=(tuple((row['path'],tuple(row.get('stamp',()))) for row in plan),str(self.store.preferences('organisation')))
        cached=self._summary_cache.get(root,{})
        if cached.get('signature')!=signature:cached={'signature':signature,'counts':{}};self._summary_cache[root]=cached
        self._preview_cache=getattr(self,'_preview_cache',{})
        if len(self._preview_cache)>=5:self._preview_cache.pop(next(iter(self._preview_cache)))
        self._preview_cache[options]=plan
        self.tools_plan=plan;self._tool_counts=cached['counts']
        self._update_all_tool_counts(plan)
        self.tools_workspace.setVisible(options[0] != 'summary');self.tools_summary_space.setVisible(options[0] == 'summary')
        self.tools_dj_only.setVisible(False)
        self.render_tools_plan();self.tools_tabs.fit_current_page();self.tools_tabs.updateGeometry()
        if options[0] in ('tags','organise'):
            if getattr(self,'_action_counts_signature',None)!=signature:
                self.update_action_counts(plan);self._action_counts_signature=signature

    def _update_all_tool_counts(self, plan=None):
        target_plan = plan if plan is not None else self.tools_plan
        signature=(tuple((r['path'],tuple(r.get('stamp',()))) for r in target_plan),str(self.store.preferences('organisation')))
        if getattr(self,'_health_counts_signature',None)==signature and hasattr(self,'_health_counts_values'):
            self._tool_counts=dict(self._health_counts_values);return
        from .maintenance import date_changes, disc_changes, track_changes, detect_superseded_singles
        from .musical_keys import key_changes
        from .library_workflows import missing_metadata
        tags_cnt = 0
        meta_cnt = 0
        art_cnt = 0
        org_cnt = 0
        from .library_workflows import workflow_plan, LYRIC_TAGS
        organised=workflow_plan(target_plan,'organise',arrange_layout=True)
        for r in organised:
            t = r.get('tags', {})
            d_edits, _ = date_changes(t)
            disc_edits, disc_issue = disc_changes(t)
            track_edits, track_issue = track_changes(t)
            k_edits, _ = key_changes(t)
            if d_edits or disc_edits or track_edits or disc_issue or track_issue or k_edits or any(k in t for k in LYRIC_TAGS):
                tags_cnt += 1
            if missing_metadata(t):
                meta_cnt += 1
            if r.get('cover_size') != (1280, 1280):
                art_cnt += 1
            if r.get('target') != r.get('path') or r.get('superseded_by') or r.get('stray_reunited'):
                org_cnt += 1
        try:
            superseded = detect_superseded_singles(target_plan)
            org_cnt = max(org_cnt, len(superseded))
        except Exception:
            pass
        self._tool_counts['tags'] = tags_cnt
        self._tool_counts['metadata'] = meta_cnt
        self._tool_counts['artwork'] = art_cnt
        self._tool_counts['organise'] = org_cnt
        self._health_counts_signature=signature;self._health_counts_values=dict(self._tool_counts)
        root = self.tools_root.currentData() if hasattr(self, 'tools_root') else None
        if root:
            cached = self._summary_cache.setdefault(root, {})
            cached['counts'] = dict(self._tool_counts);cached['signature']=signature
        if hasattr(self, 'tools_summary_buttons') and self.tools_summary_buttons:
            for key, label in [('tags', 'Correct tags'), ('metadata', 'Add missing tags'), ('artwork', 'Fix artwork'), ('organise', 'Organise files')]:
                if key in self.tools_summary_buttons:
                    count = self._tool_counts.get(key)
                    noun = 'file' if count == 1 else 'files'
                    detail = f'{count:,} {noun}' if count is not None else 'open to check'
                    self.tools_summary_buttons[key].setText(f'{label}\n{detail}')

    @Slot(str)
    def _preview_failed(self,message):
        self._preview_error=True;self._prepared_mode=None
        self.tools_status.setText(message);self.log(message);self.tools_apply_button.setEnabled(False)

    @Slot()
    def _preview_finished(self):
        worker=self._preview_worker;self._preview_worker=None
        self.scan_activity.hide()
        if self._closing_requested:
            self._preview_pending=None
            if worker:worker.deleteLater()
            return
        if worker:
            worker.wait(2000)
            worker.deleteLater()
        if getattr(self,'_after_preview_job',None):
            pending=self._after_preview_job;self._after_preview_job=None;self._preview_pending=None
            self.tools_workspace.setEnabled(True);self.job(*pending)
        elif self._preview_pending:self._start_preview()
        else:
            self.tools_workspace.setEnabled(True);self.update_tools_selection()

    def inspect_library(self):
        if self.worker:return
        if self.demo_mode:return self.demo_notice()
        from .library_workflows import inspect_snapshot
        root=self.tools_root.currentData()
        if not root:return
        if not Path(root).is_dir():
            snapshot = self._load_cached_snapshot(root)
            self.tools_snapshots[root] = snapshot
            self.tools_plan = snapshot
            self.invalidate_tools_plan()
            self._update_all_tool_counts(snapshot)
            self.render_tools_plan()
            self.tools_inspection_status.setText(f'Drive for {Path(root).name} is offline. Loaded {len(snapshot):,} cached files from database.')
            return
        snapshot=self.tools_snapshots.get(root,[])
        self._dirty_roots.add(root)
        def work(cancel,progress):
            result=scan(self.store,root,cancelled=cancel,progress=progress)
            if result['status']=='complete' and not cancel():
                paths=[r['path'] for r in self.store.rows("SELECT path FROM local_files WHERE root=? AND present=1 AND lower(path) LIKE '%.flac'",(str(root),))]
                result['inspection']=inspect_snapshot(root,snapshot,cancel,progress,self.store.preferences('organisation'),paths=paths)
                if not cancel():
                    from .freshness import save_inspection
                    save_inspection(self.store,root,result['inspection'])
            return result
        def done(result):
            self._dirty_roots.discard(root)
            if 'inspection' not in result or self.worker.isInterruptionRequested():
                self.tools_inspection_status.setText('Inspection not completed. The previous inspection is retained.');return
            plan=result['inspection'];self.tools_plan=plan;self.tools_snapshots[root]=plan;self.invalidate_tools_plan()
            self._update_all_tool_counts(plan)
            self.render_tools_plan()
            self._library_content_changed(root,refresh=False,files_changed=False)
            self.tools_inspection_status.setText(f'{len(plan):,} FLAC files checked · current as of {datetime.now():%H:%M}. Library index updated. Changed files are refreshed automatically before the next operation.')
        self.job(work,done,label='Inspect library health',local=True)

    def _library_content_changed(self,root=None,refresh=True,files_changed=True):
        """Invalidate dependent cached views after a scan/tag/move operation."""
        self._preview_cache={};self._preview_input_signature=None
        if root:
            self.linking_completed.pop(str(root),None)
            self._scan_generations[str(root)]=self._scan_generations.get(str(root),0)+1
            if files_changed:self._dirty_roots.add(str(root))
        for page_name in ('mqa_page','optimizations_page','online_optimizations_page'):
            page=getattr(self,page_name,None)
            if page and (root is None or getattr(page,'result_root',None)==str(root)):
                page.result_root=None
                page.status.setText('Library contents changed · saved results retained; refresh before acting.')
        if hasattr(self,'render_link_releases'):self.render_link_releases()
        if refresh:self.refresh()

    def render_tools_plan(self):
        from .library_workflows import has_changes,missing_metadata,workflow_plan,attention
        mode=self.tools_operation();rows=[]
        self.tools_workspace.setVisible(mode!='summary');self.tools_summary_space.setVisible(mode=='summary')
        if mode=='summary':
            if self.tools_plan and len(self._tool_counts) < 4:
                self._update_all_tool_counts()
            for key,label in [('tags','Correct tags'),('metadata','Add missing tags'),('artwork','Fix artwork'),('organise','Organise files')]:
                count=self._tool_counts.get(key)
                noun='file' if count==1 else 'files'
                detail=f'{count:,} {noun}' if count is not None else 'open to check'
                self.tools_summary_buttons[key].setText(f'{label}\n{detail}' if self.tools_plan else f'{label}\nInspect first')
            from .maintenance import first
            linked=sum(bool(r.get('linked_ids') or first(r.get('tags',{}),'tidal_track_id') and first(r.get('tags',{}),'tidal_album_id')) for r in self.tools_plan)
            bpm=sum('BPM' in missing_metadata(r.get('tags',{})) for r in self.tools_plan)
            key=sum('musical key' in missing_metadata(r.get('tags',{})) for r in self.tools_plan)
            self.tools_summary_status.setText(f'{len(self.tools_plan):,} local files inspected · choose one maintenance operation to review.' if self.tools_plan else 'No local tags loaded yet. Refresh local tags checks files without changing them.')
            if hasattr(self, 'tools_apply_button'):
                self.tools_apply_button.setText('Choose a tool above to apply…')
                self.tools_apply_button.setEnabled(False)
            return
        elif hasattr(self, 'tools_apply_button'):
            self.tools_apply_button.setText('Apply…')
            self.tools_apply_button.setEnabled(True)
        if mode == 'links':
            if isinstance(self.tools_table, VirtualTable):
                self.tools_table.model().headers = ['File', 'Link status', 'Online album match', 'Recording evidence', 'Result']
                self.tools_table.model().headerDataChanged.emit(Qt.Orientation.Horizontal, 0, 4)
            else:
                self.tools_table.setHorizontalHeaderLabels(['File', 'Link status', 'Online album match', 'Recording evidence', 'Result'])
            ignored_paths = self.store.ignored_local_files()
            row_types = {}
            for idx, row in enumerate(self.tools_plan):
                if row['path'] in ignored_paths:
                    row_types[idx] = 'ignored'
                rel_path = str(Path(row['path']).relative_to(row['root']))
                choice = row.get('catalogue_choice')
                opts = row.get('catalogue_options') or []
                linked = row.get('linked_ids') or {}
                if linked:
                    status_text = 'Linked · Chosen match' if row.get('manual') else 'Linked · Ready'
                elif opts:
                    status_text = 'Review release structure' if not any(o.get('structure',{}).get('compatible') for o in opts) else f"Needs choice ({len(opts)} options)"
                elif row.get('blocked'):
                    status_text = str(row['blocked'])
                else:
                    status_text = 'Unlinked · Not in online catalogue'
                if row['path'] in ignored_paths:
                    status_text = f"[Ignored] {status_text}"
                if choice:
                    from .linking import online_match_label
                    match_text = online_match_label(row)
                elif opts:
                    match_text = f"{len(opts)} candidates · {sum(o.get('structure',{}).get('compatible',False) for o in opts)} structurally compatible · Choose match…"
                else:
                    match_text = '—'
                if choice:
                    evidence_text = f"Track {choice.get('track_id', '')} · {choice.get('evidence', '')}"
                elif row.get('catalogue_note'):
                    evidence_text = row['catalogue_note']
                else:
                    evidence_text = '—'
                tags=row.get('tags',{})
            rows.append((rel_path, status_text, match_text, evidence_text, 'Saved in database', ', '.join(tags.get('albumartist',[])), ', '.join(tags.get('album',[]))))
            fill(self.tools_table, rows, keys=[r['path'] for r in self.tools_plan], row_types=row_types)
            self.tools_table.setColumnHidden(2, False)
            self.tools_table.setColumnHidden(3, False)
            self.tools_table.setColumnHidden(4, True)
            linked_count = sum(bool(r.get('linked_ids') or r.get('catalogue_choice')) for r in self.tools_plan)
            self.tools_status.setText(f'{len(rows):,} files · {linked_count:,} linked online. Links and verified matches save automatically in your database.')
            self.filter_tools()
            return
        if isinstance(self.tools_table, VirtualTable):
            self.tools_table.model().headers = ['File', 'Needs attention', 'Proposed changes', 'Destination', 'Result']
            self.tools_table.model().headerDataChanged.emit(Qt.Orientation.Horizontal, 0, 4)
        else:
            self.tools_table.setHorizontalHeaderLabels(['File', 'Needs attention', 'Proposed changes', 'Destination', 'Result'])
        ignored_paths = self.store.ignored_local_files()
        row_types = {}
        for idx, row in enumerate(self.tools_plan):
            p = row['path']
            if p in ignored_paths:
                row_types[idx] = 'ignored'
            changes_list = []
            if row.get('superseded_by'):
                changes_list.append('Move to Trash (redundant single)')
            if mode == 'organise' and row['target'] != row['path']:
                changes_list.append('Move file to match tag layout')
            if row.get('changes'):
                changes_list.append('; '.join(f'{key}: Remove tag' if not value else f"{key}: {', '.join(row.get('tags',{}).get(key,[])) or '(missing)'} → {', '.join(value)}" for key,value in row['changes'].items()))
            if row.get('artwork_change'):
                changes_list.append('Front cover → 1280 × 1280')
            if getattr(self, 'tools_org_duplicates', None) and self.tools_org_duplicates.isChecked() and any('duplicate track' in str(iss).casefold() for iss in row.get('issues', [])):
                changes_list.append('Manual review: duplicate track number in folder')
            changes = '; '.join(changes_list)
            issues_str = row['blocked'] or row.get('collision') or row.get('apply_error') or '; '.join(row['issues'])
            if p in ignored_paths:
                issues_str = f"[Ignored] {issues_str}" if issues_str else "[Ignored]"
            dest_str = 'Move to Trash (redundant)' if row.get('superseded_by') else (str(Path(row['target']).relative_to(row['root'])) if row['target']!=row['path'] else 'Unchanged')
            rows.append((str(Path(row['path']).relative_to(row['root'])), issues_str or 'No issues found',
                         changes or '—', dest_str, row.get('result','Preview only')))
        fill(self.tools_table,rows,keys=[r['path'] for r in self.tools_plan],row_types=row_types)
        show_dest = (mode == 'organise')
        self.tools_table.setColumnHidden(3, not show_dest)
        self.tools_table.setColumnHidden(2, False)
        self.tools_table.setColumnHidden(4, True)
        proposed = sum(bool(r.get('changes') or r.get('artwork_change') or (mode == 'organise' and r.get('target') != r.get('path')) or r.get('superseded_by')) for r in self.tools_plan if r['path'] not in ignored_paths)
        if mode == 'summary':
            attention_count = sum(bool(r.get('issues')) for r in self.tools_plan if r['path'] not in ignored_paths)
            self.tools_status.setText(f'{len(rows):,} inspected · {attention_count:,} need attention. Click a tool tab above to preview and apply changes.' if rows else 'Summary uses saved inspection. Click a tool above to work on changes.')
        else:
            self.tools_status.setText(f'{len(rows):,} inspected · {proposed:,} proposed changes. Select changes below to apply this operation only.' if rows else 'Refresh the inspection to see the current files in this library.')
        self.filter_tools()

    def tools_visible(self):
        return [r for i,r in enumerate(self.tools_plan) if not self.tools_table.isRowHidden(i)]

    def tools_scope_rows(self):
        sel = self.tools_selected()
        return sel if sel else self.tools_visible()

    def update_tools_selection(self):
        if not hasattr(self,'tools_apply_button'):return
        from .library_workflows import has_changes
        selected=self.tools_selected();scope=self.tools_scope_rows();mode=self.tools_operation()
        ignored_paths = self.store.ignored_local_files()
        if mode == 'links':
            self.tools_apply_button.setVisible(False)
            linked_cnt = sum(bool(r.get("linked_ids") or r.get("catalogue_choice")) for r in self.tools_plan)
            self.tools_scope_status.setText(f'Online linking: {linked_cnt:,} of {len(self.tools_plan):,} files linked. Links save automatically.')
        elif mode == 'summary':
            self.tools_apply_button.setVisible(True)
            self.tools_apply_button.setText('Choose a tool above to apply…')
            self.tools_apply_button.setEnabled(False)
            self.tools_scope_status.setText('Summary of all library issues. Choose a tool tab above to preview and apply changes.')
        else:
            self.tools_apply_button.setVisible(True)
            scope_desc = f'{len(selected):,} selected' if selected else f'{len(scope):,} visible'
            self.tools_scope_status.setText(f'Operations affect {scope_desc} files. Applying changes uses the selected proposals.')
            if mode == 'tags':
                actionable = [r for r in selected if r.get('changes') and not r.get('blocked') and not r.get('collision') and r['path'] not in ignored_paths]
            elif mode == 'organise':
                actionable = [r for r in selected if (r['target'] != r['path'] or r.get('superseded_by') or r.get('changes')) and not r.get('blocked') and not r.get('collision') and r['path'] not in ignored_paths]
            else:
                actionable = [r for r in selected if has_changes(r) and not r.get('blocked') and not r.get('collision') and r['path'] not in ignored_paths]
            verbs = {'tags': 'Update tags', 'organise': 'Organise files', 'metadata': 'Add missing tags', 'artwork': 'Apply covers', 'links': 'Apply'}
            self.tools_apply_button.setText(f'{verbs.get(mode, "Apply")} ({len(actionable):,})…');self.tools_apply_button.setEnabled(bool(actionable) and self.worker is None and self._preview_worker is None and self._preview_pending is None and not self._preview_error)
        if hasattr(self, 'link_choose_btn'):
            has_opts = any(bool(r.get('catalogue_options')) for r in selected)
            self.link_choose_btn.setEnabled(has_opts)
        lines=[]
        for row in selected[:1]:
            tags=row.get('tags',{})
            lines.append(f"📁 Local File: {row['path']}")
            lines.append(f"   • Album artist (library grouping): {', '.join(tags.get('albumartist',[])) or '(empty)'}")
            lines.append(f"   • Track artist (performer credits): {', '.join(tags.get('artist',[])) or '(empty)'}")
            lines.append(f"   • Album: {', '.join(tags.get('album',[])) or '(empty)'} · Title: {', '.join(tags.get('title',[])) or '(empty)'}")
            tid = ', '.join(tags.get('tidal_track_id',[])) or '(none)'
            aid = ', '.join(tags.get('tidal_album_id',[])) or '(none)'
            lines.append(f"   • Saved Online IDs: track {tid} · release {aid}")
            
            linked = row.get('linked_ids',{})
            if linked or row.get('catalogue_choice'):
                choice = row.get('catalogue_choice',{})
                match_name = f"{choice.get('artist')} — {choice.get('album')}" if choice else "Online Match"
                rel_id = linked.get('album_id') or (choice.get('id') if choice else '—')
                trk_id = linked.get('track_id') or (choice.get('track_id') if choice else '—')
                lines.append(f"🌐 Online Online Link: {match_name} (Release {rel_id} · Track {trk_id})")
            seen_dj = set()
            for check in row.get('dj_checks',[]):
                dj_line = f"   • DJ Analysis: BPM {', '.join(check.get('bpm',[])) or '—'} · Key {', '.join(check.get('key',[])) or '—'}"
                if dj_line not in seen_dj:
                    seen_dj.add(dj_line)
                    lines.append(dj_line)
            
            if mode != 'links' and row.get('changes'):
                lines.append("🏷 Proposed Tag Changes:")
                for key,values in row['changes'].items():
                    local_val = ', '.join(tags.get(key,[])) or '(missing)'
                    lines.append(f"   • {key}: {local_val} → {', '.join(values) if values else 'Remove tag'}")
            if row.get('artwork_change'):
                lines.append(f"   • Front cover: {row.get('cover_size') or 'none'} → 1280 × 1280")
            if row['target']!=row['path']:
                lines.append(f"   • Destination: {row['target']}")
                if mode=='organise':
                    from .organisation import path_tag_notes
                    lines.extend(path_tag_notes(tags,row.get('layout')))
            if row.get('issues'):
                lines.append(f"⚠️ Issues: {'; '.join(row['issues'])}")
            if mode == 'links' and row.get('catalogue_options'):
                lines.append(f"ℹ️ Multiple Placements Available ({len(row['catalogue_options'])}): Use 'Choose match for selected…' or right-click to pick.")
            elif row.get('catalogue_note'):
                lines.append(f"ℹ️ Catalogue: {row['catalogue_note']}")
            if row.get('apply_error'):
                lines.append(f"❌ Error: {row['apply_error']}")
            if not has_changes(row) and not row.get('issues'):
                lines.append("✓ No changes needed for this operation.")
        self.tools_detail.setPlainText('\n'.join(lines))

    def tools_selected(self):
        if hasattr(self, 'link_table') and getattr(self, 'nav', None) and self.nav.currentRow() == 2:
            sel = self.link_selected()
            if sel: return sel
        if not hasattr(self, 'tools_table') or not self.tools_plan:
            return []
        indexes = self.tools_table.selectionModel().selectedRows()
        if not indexes:
            return []
        by_path = {r.get('path'): r for r in self.tools_plan if r.get('path')}
        model = self.tools_table.model()
        selected = []
        for idx in indexes:
            r = idx.row()
            if self.tools_table.isRowHidden(r):
                continue
            path = None
            if hasattr(model, 'keys') and 0 <= r < len(model.keys):
                path = model.keys[r]
            elif self.tools_table.item(r, 0):
                path = self.tools_table.item(r, 0).data(Qt.ItemDataRole.UserRole)
            if path and path in by_path:
                selected.append(by_path[path])
            elif 0 <= r < len(self.tools_plan):
                selected.append(self.tools_plan[r])
        return selected

    def review_artist_files(self):
        if self.worker or self.demo_mode: return
        tracks = [track for index in self.artist_table.selectionModel().selectedRows()
                  for track in self.artist_rows[source_row(self.artist_table,index.row())][1]]
        roots = self.store.rows('SELECT path,root FROM local_files WHERE present=1')
        by_path = {r['path']:r['root'] for r in roots}
        paths = [t['path'] for t in tracks if t['path'].lower().endswith('.flac') and t['path'] in by_path]
        if not paths: return
        root = by_path[paths[0]]
        # One repair plan always belongs to one library root.
        paths = [p for p in paths if by_path[p] == root]
        cached = {r['path']:r for r in self.tools_snapshots.get(root, [])}
        def work(cancel, progress):
            result = []
            for path in paths:
                if cancel(): break
                from .library_workflows import inspect_snapshot
                result.extend(inspect_snapshot(root,[cached[path]] if path in cached else [],cancel,progress,self.store.preferences('organisation'),paths=[path]))
            return result
        def done(plan):
            self.nav.setCurrentRow(2)
            if hasattr(self, 'link_releases_root'):
                self.link_releases_root.setCurrentIndex(self.link_releases_root.findData(root))
            self.tools_root.setCurrentIndex(self.tools_root.findData(root))
            combined = {r['path']:r for r in self.tools_snapshots.get(root, [])}
            combined.update({r['path']:r for r in plan})
            self.tools_plan = list(combined.values());self.tools_snapshots[root] = self.tools_plan
            if hasattr(self, 'render_link_releases'):
                self.render_link_releases()
            if hasattr(self, 'link_search'):
                self.link_search.clear()
            selected_paths = set(paths)
            if hasattr(self, 'link_table'):
                self.link_table.clearSelection()
                for index,row in enumerate(self.link_plan):
                    if row['path'] in selected_paths:
                        self.link_table.selectionModel().select(self.link_table.model().index(index,0),QItemSelectionModel.SelectionFlag.Select|QItemSelectionModel.SelectionFlag.Rows)
            self.link_status.setText(f'{len(plan)} artist files inspected. Use Start / resume linking or Choose match for selected…')
        self.job(work,done,label='Inspect artist files')

    def filter_tools(self):
        if not hasattr(self,'tools_table'):return
        from .library_workflows import attention,has_changes,missing_metadata
        from .link_statistics import link_statistics,matches_link_filter
        query=self.tools_search.text().casefold();mode=self.tools_operation()
        active=link_statistics(self.store,self.market)['active'] if mode=='links' else {}
        filter_text = self.tools_filter.currentText()
        ignored_paths = self.store.ignored_local_files()
        by_path = {r['path']: r for r in self.tools_plan}
        for index in range(self.tools_table.rowCount()):
            cell = self.tools_table.item(index, 0)
            row = by_path.get(cell.data(Qt.ItemDataRole.UserRole)) if cell else None
            if row is None: continue
            text=' '.join(self.tools_table.item(index,col).text() for col in range(self.tools_table.columnCount()) if self.tools_table.item(index,col))
            hidden=bool(query and query not in text.casefold())
            p = row.get('path')
            is_ignored = p in ignored_paths
            if filter_text in ('Needs attention', 'Needs cleanup / fixes'):
                hidden |= is_ignored or not (attention(row) or has_changes(row) or row.get('superseded_by'))
            elif filter_text in ('Proposed changes', 'Proposed tag changes', 'Proposed artwork changes'):
                has_proposed = has_changes(row) or bool(row.get('changes')) or bool(row.get('artwork_change')) or row.get('target', p) != p or bool(row.get('superseded_by'))
                if getattr(self, 'tools_org_duplicates', None) and self.tools_org_duplicates.isChecked() and any('duplicate track' in str(iss).casefold() for iss in row.get('issues', [])):
                    has_proposed = True
                hidden |= is_ignored or not has_proposed
            elif filter_text == 'Unstandardised dates':
                hidden |= is_ignored or not any('date' in str(iss).casefold() for iss in row.get('issues', []))
            elif filter_text == 'Disc numbers':
                hidden |= is_ignored or not any('disc' in str(iss).casefold() for iss in row.get('issues', []))
            elif filter_text == 'Missing musical key':
                hidden |= is_ignored or 'musical key' not in missing_metadata(row.get('tags',{}))
            elif filter_text == 'Missing BPM':
                hidden |= is_ignored or 'BPM' not in missing_metadata(row.get('tags',{}))
            elif filter_text == 'Duplicate track numbers':
                hidden |= is_ignored or not any('duplicate track' in str(iss).casefold() for iss in row.get('issues', []))
            elif filter_text in ('Stray tracks (mixed artist)', 'Stray tracks (mixed album artist)'):
                hidden |= is_ignored or not (bool(row.get('stray_reunited')) or any('stray track' in str(iss).casefold() for iss in row.get('issues', [])))
            elif filter_text == 'Folder moves only':
                hidden |= is_ignored or (row.get('target', p) == p and not row.get('superseded_by'))
            elif filter_text == 'Superseded singles':
                hidden |= is_ignored or not bool(row.get('superseded_by'))
            elif filter_text in ('Unlinked tracks','Linked tracks','Too many editions / Needs choice'):
                hidden |= not matches_link_filter(row,filter_text,active,ignored_paths)
            elif filter_text == 'Ignored files':
                hidden |= not is_ignored
            if mode=='metadata' and self.tools_dj_only.isChecked():hidden |= not any(x in missing_metadata(row.get('tags',{})) for x in ('BPM','musical key'))
            self.tools_table.setRowHidden(index,hidden)
        self.update_tools_selection()

    def rematch_changed_artists(self,names,cancel,progress):
        from .core import is_compilation_artist
        names={name for name in names if name and not is_compilation_artist(name)}
        saved={r['artist'] for r in self.store.rows("SELECT artist FROM mappings WHERE manual=1 OR status IN ('auto','confirmed')")}
        names-=saved
        if not names or cancel() or self.demo_mode:return
        if not self.credentials.session:
            progress('Artist tags refreshed. Connect Online to retry unresolved artist links; recording checks can use ISRC or saved IDs.');return
        local=self.store.artists();artists=[(name,local[name]) for name in sorted(names) if name in local]
        if not artists:return
        progress(f'Rechecking {len(artists)} unresolved album artists after tag changes')
        try:
            matcher=BatchMatcher(self.store,self.api(cancel,progress),self.market,self.load_favourites,redact=self.credentials.redact)
            progress(matcher.run(artists,cancel,progress,resume=True))
        except (CatalogueError,CredentialError) as exc:
            progress('Artist relinking paused · '+str(exc)+' · saved tag changes retained')

    def _check_background_linking(self):
        now_at=time.monotonic()
        if now_at-getattr(self,'_last_background_link_check',0)<2.0:
            return
        self._last_background_link_check=now_at
        if (getattr(self, 'demo_mode', False) or getattr(self, 'worker', None)
                or getattr(self, '_link_worker', None) or getattr(self,'is_linking_active',False)):
            return
        if self.settings.value('background_linking_paused', False, type=bool):
            return
        if self._bg_probe_worker:return
        candidate_roots = []
        if hasattr(self, 'link_releases_root') and self.link_releases_root.currentData():
            candidate_roots.append(self.link_releases_root.currentData())
        if hasattr(self, 'get_checked_roots'):
            for cr in self.get_checked_roots():
                if cr not in candidate_roots:
                    candidate_roots.append(cr)
        if hasattr(self, 'root_rows'):
            for rr in self.root_rows:
                if rr['root'] not in candidate_roots:
                    candidate_roots.append(rr['root'])
        if not candidate_roots:
            for r in self.store.roots():
                if r['root'] not in candidate_roots:
                    candidate_roots.append(r['root'])
        snapshots={root:self.tools_snapshots.get(root) or self._load_cached_snapshot(root) for root in candidate_roots}
        snapshots={root:rows for root,rows in snapshots.items() if rows}
        if not snapshots:return
        def probe(cancel,progress):
            from .maintenance import first
            from .linking import pending_paths,state_signature,attach_links
            resolved={r['artist'] for r in self.store.rows("SELECT artist FROM mappings WHERE status IN ('auto','confirmed') AND tidal_id IS NOT NULL")}
            for root,snapshot in snapshots.items():
                if cancel():return None
                attach_links(snapshot,self.store,self.market)
                eligible=[r for r in snapshot
                    if (r.get('recheck_required') or not (r.get('linked_ids') or (first(r.get('tags',{}),'tidal_track_id') and first(r.get('tags',{}),'tidal_album_id'))))
                    and (first(r.get('tags',{}),'albumartist') or first(r.get('tags',{}),'artist') or '') in resolved]
                if pending_paths(self.store,self.market,eligible):return root,state_signature(self.store,self.market,root)
            return None
        self._bg_probe_worker=Worker(probe)
        self._bg_probe_worker.result.connect(self._background_probe_ready,Qt.ConnectionType.QueuedConnection)
        self._bg_probe_worker.finished.connect(self._background_probe_finished,Qt.ConnectionType.QueuedConnection)
        self._bg_probe_worker.start()

    @Slot(object)
    def _background_probe_ready(self,result):
        if self._closing_requested or not result or self.worker or self._link_worker or self.is_linking_active:return
        root,signature=result
        if self.linking_completed.get(root)==signature:return
        if hasattr(self,'link_releases_root'):
            idx=self.link_releases_root.findData(root)
            if idx>=0:self.link_releases_root.setCurrentIndex(idx)
        self.start_linking(bg=True)

    @Slot()
    def _background_probe_finished(self):
        worker=self._bg_probe_worker;self._bg_probe_worker=None
        if worker:worker.deleteLater()

    def recheck_selected_tracks(self):
        paths={row['path'] for row in self.link_selected()}
        if not paths:
            self.link_status.setText('Select tracks to search for matches.');return
        self.start_linking(recheck=True,target_paths=paths)

    def recheck_unlinked_tracks(self):
        from .link_statistics import unresolved_paths
        paths=unresolved_paths(self.store,self.market,getattr(self,'link_plan',[]))
        self.link_filter.setCurrentText('Unlinked tracks')
        if not paths:
            self.link_status.setText('No unresolved tracks in this library.');return
        self.start_linking(recheck=True,target_paths=paths)

    def recheck_multiple_editions(self):
        if not hasattr(self, 'link_plan'): return
        from .link_statistics import unresolved_paths
        multiple=unresolved_paths(self.store,self.market,self.link_plan,editions_only=True)
        if not multiple:
            self.link_status.setText('No tracks currently have multiple editions or ambiguous placements.')
            return
        self.start_linking(recheck=True, target_paths=set(multiple))

    def link_release_id(self):
        import re
        from PySide6.QtWidgets import QInputDialog
        selected = self.link_selected()
        if not selected:
            QMessageBox.information(self, 'Selection required', 'Select one or more tracks to link with a release ID.')
            return
        raw, ok = QInputDialog.getText(self, 'Link Release ID', 'Release ID or web URL:')
        if not ok or not raw.strip(): return
        match = re.search(r'(\d+)', raw.strip())
        if not match:
            QMessageBox.warning(self, 'Invalid ID or URL', 'Please enter a numeric ID or a URL containing digits (e.g. 67890 or https://tidal.com/album/67890).')
            return
        rel_id = match.group(1)
        if self.demo_mode: return self.demo_notice()
        def work(cancel, progress):
            api = self.api(cancel, progress)
            rel = api.album(rel_id, progress)
            if not rel:
                raise CatalogueError(f'Release {rel_id} not found online.')
            from .linking import save_result
            for r in selected:
                opt = dict(rel, album_id=rel_id, id=rel_id, artist=rel.get('artist', ''))
                r['catalogue_choice'] = opt
                save_result(self.store, self.market, r, manual=True)
            return f'Linked {len(selected)} track(s) to release {rel_id} ({rel.get("title", "")})'
        def done(msg):
            self.log(msg)
            self.invalidate_tools_plan()
            if hasattr(self, 'render_link_releases'):
                self.render_link_releases()
            self.link_status.setText(msg)
        self.job(work, done, label=f'Link release ID · {rel_id}')

    def start_linking(self, checked=False, recheck=False, target_paths=None, bg=False):
        if self._closing_requested or self.demo_mode or self.worker or self._link_worker or self.is_linking_active:return
        root = self.link_releases_root.currentData() if hasattr(self, 'link_releases_root') and self.link_releases_root.currentData() else self.tools_root.currentData()
        if not root:
            if not bg: self.link_status.setText('Choose a library first.')
            return
        import copy
        snapshot = self.tools_snapshots.get(root, self.tools_plan)
        if not snapshot:
            snapshot = self._load_cached_snapshot(root)
            self.tools_snapshots[root] = snapshot
        snapshot = copy.deepcopy(snapshot)
        if target_paths:
            selected = set(target_paths)
        elif recheck:
            selected = {r['path'] for r in (self.link_selected() if (hasattr(self, 'nav') and self.nav.currentRow() == 2) else self.tools_selected())}
            if not selected:
                self.link_status.setText('Select the files whose links you want to recheck.');return
        else:
            selected = None
        def work(cancel,progress):
            if cancel(): return 'Linking paused · saved results retained'
            from .freshness import prepare_library
            from .linking import link_recordings,attach_links
            from .dj_metadata import DJMetadata
            from .maintenance import first
            if snapshot and root not in getattr(self, '_dirty_roots', set()):
                fresh = snapshot
            else:
                fresh = prepare_library(self.store, root, snapshot, cancel, progress)
                if hasattr(self, '_dirty_roots'):
                    self._dirty_roots.discard(root)
            if cancel(): return 'Linking paused · saved results retained'
            attach_links(fresh, self.store, self.market)
            from .link_statistics import unresolved_paths
            unresolved=unresolved_paths(self.store,self.market,fresh) if selected is None else None
            if bg:
                resolved_artists = set()
                try:
                    resolved_artists = {
                        r['artist'] for r in self.store.rows(
                            "SELECT artist FROM mappings WHERE status IN ('auto','confirmed') AND tidal_id IS NOT NULL"
                        )
                    }
                except Exception:
                    pass
                chosen = [
                    r for r in fresh
                    if r['path'] in unresolved
                    and (first(r.get('tags', {}), 'albumartist') or first(r.get('tags', {}), 'artist') or '') in resolved_artists
                ]
                if not chosen:
                    return 'No unlinked tracks for confirmed artists to link.'
            else:
                chosen=[r for r in fresh if r['path'] in (unresolved if selected is None else selected)]
            if not chosen:return 'No unresolved tracks to link.'
            names={first(r.get('tags',{}),'albumartist') for r in chosen}
            changed={first(r.get('tags',{}),'albumartist') for r in chosen if r.get('needs_artist_match')}
            previous={r['artist'] for r in self.store.rows('SELECT artist FROM match_reviews')}
            confirmed={r['artist'] for r in self.store.rows("SELECT artist FROM mappings WHERE manual=1 OR status='confirmed'")}
            local=self.store.artists()
            artists=[(name,local[name]) for name in sorted(names) if name in local and name not in confirmed and (recheck or name not in previous or name in changed)]
            api=self.api(cancel,progress)
            if artists and not bg and selected is None:
                progress('Step 1 of 2 · matching Album Artists and caching their releases')
                matcher=BatchMatcher(self.store,api,self.market,self.load_favourites,redact=self.credentials.redact)
                progress(matcher.run(artists,cancel,progress,resume=not recheck))
            if cancel():return 'Linking paused · saved results retained'
            progress(f'Searching matches for {len(chosen):,} selected tracks · other files provide release context only' if selected is not None else 'Linking releases and recordings · cached evidence first')
            if not recheck:attach_links(chosen,self.store,self.market)
            saved_batch=[];last_emit=[time.monotonic()]
            def record_saved(row):
                saved_batch.append(row)
                current=time.monotonic()
                if len(saved_batch)>=25 or current-last_emit[0]>=.2:
                    self._link_worker.records_saved.emit(saved_batch[:]);saved_batch.clear();last_emit[0]=current
            with DJMetadata(self.store,cancel,progress,self.request_pacer,market=self.market) as dj:
                message=link_recordings(chosen,self.store,self.market,api,lambda track,album_id=None:dj.lookup(track,album_id,force=False),cancel,progress,recheck,context_rows=fresh,on_result=record_saved)
            if saved_batch:self._link_worker.records_saved.emit(saved_batch[:])
            if cancel(): return 'Linking paused · saved results retained'
            return dict(message=message,root=root,rows=attach_links(fresh,self.store,self.market))
        if not bg:
            self._link_enabled=selected is None
            self.settings.setValue('background_linking_paused',selected is not None)
        if getattr(self, 'session_link_start_time', None) is None:
            self.session_link_start_time = time.monotonic()
        self.set_job_actions_busy(True)
        self.link_start.setEnabled(False);self.link_pause.setEnabled(True)
        self.link_status.setText('Linking in background · local cleanup remains available')
        self.background_link_indicator.show()
        if hasattr(self, 'online_status_label'):
            self.online_status_label.setText('☁️ Linking releases in background')
        if hasattr(self, 'activity_nav_btn') and not getattr(self, 'worker', None):
            self.activity_nav_btn.setText('Activity (Linking…)')
        if hasattr(self, 'nav') and self.nav.item(8) and not getattr(self, 'worker', None):
            self.nav.item(8).setText('⚡ Activity (Linking…)')
        if hasattr(self, 'activity_job_heading') and not getattr(self, 'worker', None):
            self.activity_job_icon.setText('🔄')
            self.activity_job_heading.setText('Running: Linking releases and tracks in background')
            self.activity_job_detail.setText('Querying Online catalogue…')
            if hasattr(self, 'activity_pause_link_btn'):
                self.activity_pause_link_btn.setText('Pause background linking')
                self.activity_pause_link_btn.setVisible(True)
                self.activity_pause_link_btn.setEnabled(True)
        self._link_worker=Worker(work)
        self.is_linking_active=True
        self._link_worker.message.connect(self.link_progress,Qt.ConnectionType.QueuedConnection)
        self._link_worker.record_saved.connect(self.link_row_saved,Qt.ConnectionType.QueuedConnection)
        self._link_worker.records_saved.connect(self.link_rows_saved,Qt.ConnectionType.QueuedConnection)
        self._link_worker.result.connect(self.link_result,Qt.ConnectionType.QueuedConnection)
        self._link_worker.failure.connect(self.link_failure,Qt.ConnectionType.QueuedConnection)
        self._link_worker.finished.connect(self.linking_finished,Qt.ConnectionType.QueuedConnection)
        self._link_worker.start()

    @Slot(object)
    def link_row_saved(self,row):
        # Update only the affected visible record, without rescanning the library.
        path=row.get('path')
        current=next((r for r in getattr(self,'link_plan',[]) if r['path']==path),None)
        if current is None or current.get('stamp')!=row.get('stamp'):return
        current.update(row)
        model=self.link_table.model()
        try:index=model.keys.index(path)
        except ValueError:return
        choice=row.get('catalogue_choice') or {};linked=bool(row.get('linked_ids') or choice)
        model.rows[index]=list(model.rows[index])
        model.rows[index][1]='Linked · Ready' if linked else 'Needs review'
        from .linking import online_match_label
        model.rows[index][2]=online_match_label(row) if linked else 'Choose a release placement'
        model.rows[index][3]=row.get('catalogue_note','')
        model.dataChanged.emit(model.index(index,0),model.index(index,model.columnCount()-1))
        if linked and self.link_filter.currentText() in ('Needs attention','Unlinked tracks','Too many editions / Needs choice'):
            self.link_table.setRowHidden(index,True)

    @Slot(object)
    def link_rows_saved(self,rows):
        self._action_counts_signature=None;self._health_counts_signature=None
        self._preview_cache={};self._preview_input_signature=None
        for row in rows or ():self.link_row_saved(row)

    @Slot(object)
    def link_result(self,result):
        if isinstance(result,str):self.link_progress(result);return
        root=result['root']
        self._preview_cache={};self._preview_input_signature=None
        self.tools_snapshots[root]=result['rows']
        from .linking import state_signature
        self.linking_completed[root]=state_signature(self.store,self.market,root)
        self.link_progress(result['message'])

    @Slot(str)
    def link_progress(self,message):
        routine = ('Loading track details', 'Checking availability', 'Fetching ', 'Catalogue request ·',
                   'Using cached release summaries', 'Loading release summaries', 'Finished tracks', 'Finished albums')
        if message.startswith(routine) and not any(word in message.casefold() for word in ('timeout','timed out','retry','failed','error','429')):
            return
        self.link_status.setText(message)
        self.background_link_indicator.setToolTip(message)
        self.log(message)
        self._update_session_linking_progress(message)

    def toggle_link_pause(self):
        if self._link_worker and self._link_worker.isRunning() and not self._link_worker.isInterruptionRequested():
            self.pause_linking()
        else:
            self.start_linking()

    def _update_session_linking_progress(self, message):
        import re
        if getattr(self, 'session_link_start_time', None) is None:
            self.session_link_start_time = time.monotonic()
        if not hasattr(self, 'session_linked_count'):
            self.session_linked_count = 0
        if not hasattr(self, 'session_total_to_check'):
            self.session_total_to_check = 0

        m_init = re.search(r'(\d+[\d,]*)\s+to check', message)
        if m_init:
            self.session_total_to_check = int(m_init.group(1).replace(',', ''))

        m_track = re.search(r'Checking track (\d+)/(\d+)', message)
        if m_track:
            self.session_linked_count = int(m_track.group(1))
            self.session_total_to_check = int(m_track.group(2))
        else:
            m_linked = re.search(r'(?:Linked|Needs review) \((\d+[\d,]*)/(\d+[\d,]*)\)', message)
            if m_linked:
                self.session_linked_count = int(m_linked.group(1).replace(',', ''))
                self.session_total_to_check = int(m_linked.group(2).replace(',', ''))

        elapsed = time.monotonic() - self.session_link_start_time
        mins = int(elapsed // 60)
        secs = int(elapsed % 60)
        elapsed_str = f"{mins}:{secs:02d}"

        eta_str = None
        if elapsed >= 3 and self.session_linked_count > 0:
            rate = (self.session_linked_count / elapsed) * 60
            remaining = max(0, self.session_total_to_check - self.session_linked_count) if self.session_total_to_check else 0
            if rate > 0 and remaining > 0:
                eta_secs = remaining / (rate / 60)
                eta_hrs = int(eta_secs // 3600)
                eta_mins = int((eta_secs % 3600) // 60)
                eta_str = f"~{eta_hrs}h {eta_mins:02d}m" if eta_hrs > 0 else f"~{eta_mins}m"
                eta_display = f"⏱️ {elapsed_str} elapsed · {self.session_linked_count:,} checked ({rate:.1f} tracks/min) · ETA: {eta_str} ({remaining:,} remaining)"
            else:
                eta_display = f"⏱️ {elapsed_str} elapsed · {self.session_linked_count:,} checked ({rate:.1f} tracks/min)"
        else:
            eta_display = f"⏱️ {elapsed_str} elapsed · calculating speed and finish ETA…"

        if hasattr(self, 'activity_job_heading') and not getattr(self, 'worker', None):
            self.activity_job_icon.setText('🔄')
            self.activity_job_heading.setText('Running: Linking releases and tracks in background')
            self.activity_job_detail.setText(message)
            if hasattr(self, 'activity_job_eta'):
                self.activity_job_eta.setText(eta_display)
                self.activity_job_eta.setVisible(True)
            if hasattr(self, 'activity_pause_link_btn'):
                self.activity_pause_link_btn.setText('Pause background linking')
                self.activity_pause_link_btn.setVisible(True)
                self.activity_pause_link_btn.setEnabled(True)

        if hasattr(self, 'overview_session_banner') and hasattr(self, 'overview_session_banner_label'):
            if self.session_total_to_check > 0:
                progress_part = f"{self.session_linked_count:,} of {self.session_total_to_check:,} tracks checked"
            elif self.session_linked_count > 0:
                progress_part = f"{self.session_linked_count:,} tracks checked"
            else:
                progress_part = "checking tracks"
            eta_part = f"ETA: {eta_str}" if eta_str else "calculating ETA…"
            self.overview_session_banner_label.setText(
                f"🔄 Linking in background · ⏱️ {elapsed_str} elapsed · {progress_part} · {eta_part}"
            )
            self.overview_session_banner.setVisible(True)


    @Slot(str)
    def link_failure(self,message):
        self._link_enabled=False;self._link_restart=False
        # Network/auth failures require an explicit retry; the periodic probe
        # must not turn one failure into a recurring background loop.
        self.settings.setValue('background_linking_paused',True)
        self.link_progress('Linking paused · '+message+' · completed results are saved; Start / resume retries unfinished work')

    def pause_linking(self):
        self._link_enabled=False;self._link_restart=False
        self.settings.setValue('background_linking_paused', True)
        if self._link_worker:
            self._link_worker.requestInterruption()
            msg = 'Pausing after the current request · completed results are saved'
            self.link_status.setText(msg)
            if hasattr(self, 'activity_job_heading') and not getattr(self, 'worker', None):
                self.activity_job_icon.setText('⏸️')
                self.activity_job_heading.setText('Background linking paused')
                self.activity_job_detail.setText(msg)
                if hasattr(self, 'activity_pause_link_btn'):
                    self.activity_pause_link_btn.setText('Resume background linking')
                    self.activity_pause_link_btn.setVisible(True)
                    self.activity_pause_link_btn.setEnabled(True)
            if hasattr(self, 'overview_session_banner') and hasattr(self, 'overview_session_banner_label'):
                self.overview_session_banner_label.setText('⏸️ Background linking paused · completed results are saved')

    @Slot()
    def linking_finished(self):
        self.is_linking_active=False
        worker=self._link_worker;self._link_worker=None
        if worker:
            worker.wait(2000)
            worker.deleteLater()
        self.set_job_actions_busy(False)
        self.link_start.setEnabled(True);self.link_pause.setEnabled(False);self.background_link_indicator.hide()
        if hasattr(self, 'online_status_label'):
            self.online_status_label.setText('☁️ Online API: Ready')
        if hasattr(self, 'activity_nav_btn') and not getattr(self, 'worker', None):
            self.activity_nav_btn.setText('Activity')
        if hasattr(self, 'nav') and self.nav.item(8) and not getattr(self, 'worker', None):
            self.nav.item(8).setText('📋  Activity')
        if hasattr(self, 'activity_job_heading') and not getattr(self, 'worker', None):
            self.activity_job_icon.setText('⚡')
            self.activity_job_heading.setText('No background tasks currently running')
            self.activity_job_detail.setText('Ready for operations.')
            if hasattr(self, 'activity_job_eta'):
                self.activity_job_eta.setVisible(False)
            if hasattr(self, 'activity_pause_link_btn'):
                self.activity_pause_link_btn.setVisible(False)
        if hasattr(self, 'overview_session_banner'):
            self.overview_session_banner.setVisible(False)
        if not self.worker:
            self.invalidate_tools_plan();self.refresh()
            if hasattr(self, 'render_link_releases'):
                self.render_link_releases()
            self.resume_changed_links()
        deferred=self._deferred_job;self._deferred_job=None
        if deferred and not self._closing_requested:
            QTimer.singleShot(0,lambda args=deferred:self.job(*args[0],**args[1]))

    def update_live_links_cache(self):
        if not getattr(self, '_view_data', None) or not self._view_data.get('artists'):
            return
        market = getattr(self, 'market', 'GB')
        try:
            indexed = {r['path']: (r['size'], r['mtime']) for r in self.store.rows('SELECT path,size,mtime FROM local_files WHERE present=1')}
            associations = {}
            for record in self.store.rows('SELECT path,stamp,payload FROM track_links WHERE market=?', (market,)):
                stamp = json.loads(record['stamp'])
                payload = json.loads(record['payload'])
                if len(stamp) >= 4 and indexed.get(record['path']) == (stamp[2], stamp[3]) and payload.get('ids'):
                    associations[record['path']] = payload['ids']

            artists = self._view_data['artists']
            for tracks in artists.values():
                for track in tracks:
                    ids = associations.get(track['path'])
                    if ids:
                        track.update(tidal_album_id=ids.get('album_id'), tidal_track_id=ids.get('track_id'))

            from .link_statistics import link_statistics
            stats=link_statistics(self.store,self.market)
            self._view_data['link_statistics']=stats
            count=stats['track_count'];release_count=stats['release_count']
            linked=stats['linked_tracks'];resolved_releases=stats['linked_releases']

            if hasattr(self, 'card_tracks_metric') and count:
                track_pct = round(100 * linked / count)
                self.card_tracks_metric.setText(f'{linked:,} / {count:,} resolved')
                if not (self._link_worker and self._link_worker.isRunning()):
                    self.card_tracks_sub.setText(f'{track_pct}% linked to online recordings')
            if hasattr(self, 'card_releases_metric') and release_count:
                release_pct = round(100 * resolved_releases / release_count)
                self.card_releases_metric.setText(f'{resolved_releases:,} / {release_count:,} resolved')
                self.card_releases_sub.setText(f'{release_pct}% with verified album IDs')
        except Exception:
            pass

    def resume_changed_links(self):
        if self._link_enabled and self._link_restart and not self._link_worker and not self.worker:
            self._link_restart=False
            QTimer.singleShot(0,self.start_linking)

    def review_saved_links(self):
        from .repair_review import AlbumRepairReview
        from .linking import save_result
        if hasattr(self, 'nav') and self.nav.currentRow() == 2 and hasattr(self, 'link_plan'):
            rows = [r for r in self.link_plan if r.get('catalogue_options')]
        else:
            rows = [r for r in self.tools_scope_rows() if r.get('catalogue_options')]
        if not rows:
            self.link_status.setText('No saved choices in this scope. Start linking first.');return
        dialog=AlbumRepairReview(rows,self)
        if dialog.exec()!=QDialog.DialogCode.Accepted:return
        for row in dialog.plans:save_result(self.store,self.market,row,manual=True)
        self.invalidate_tools_plan()
        if hasattr(self, 'render_link_releases'):
            self.render_link_releases()
        self.link_status.setText('Choices saved to the database. Review proposed online tag corrections before applying them.')

    def fill_library_tags(self):
        if self.worker or self.demo_mode:return
        from .tag_review import check_album_tags
        root=self.tools_root.currentData();mode=self.tools_operation()
        if not root or mode not in ('metadata','artwork'):return
        selected=self.tools_scope_rows()
        if not selected:
            self.tools_status.setText('Refresh local tags, then choose visible files or select the files to check.');return
        snapshot=self.tools_snapshots.get(root,self.tools_plan)
        paths={r['path'] for r in selected};known={r['path'] for r in snapshot}
        include_new=self.tools_scope.currentIndex()==0 and not self.tools_search.text().strip() and not self.tools_dj_only.isChecked()
        def work(cancel,progress):
            from .dj_metadata import DJMetadata
            from .freshness import prepare_library
            fresh=prepare_library(self.store,root,snapshot,cancel,progress) if not snapshot or root in self._dirty_roots else copy.deepcopy(snapshot)
            from .linking import attach_links
            attach_links(fresh,self.store,self.market)
            chosen=[r for r in fresh if r['path'] in paths or (include_new and r['path'] not in known)]
            from .maintenance import first
            self.rematch_changed_artists([first(r.get('tags',{}),'albumartist') for r in chosen if r.get('needs_artist_match')],cancel,progress)
            with DJMetadata(self.store,cancel,progress,self.request_pacer,market=self.market) as dj:
                if mode=='metadata':
                    from .linking import link_recordings
                    progress(link_recordings(chosen,self.store,self.market,self.api(cancel,progress),dj.lookup,cancel,progress,context_rows=fresh))
                    checked=attach_links(chosen,self.store,self.market)
                else:
                    checked=check_album_tags(chosen,self.store,self.market,self.api(cancel,progress),cancel,progress,covers=True)
            updates={r['path']:r for r in checked}
            for row in fresh:
                result=updates.get(row['path'])
                if result is None:continue
                if mode=='metadata':
                    row['metadata_changes']=result.get('metadata_changes',{});row['metadata_note']=result.get('catalogue_note','')
                else:
                    row.pop('artwork_change',None);row.pop('artwork_proposal',None)
                    if result.get('artwork_change'):row['artwork_proposal']=result['artwork_change']
                    row['artwork_note']=result.get('catalogue_note','')
            return fresh
        def done(rows):
            self.tools_plan=rows;self.tools_snapshots[root]=rows;self.invalidate_tools_plan()
            self.tools_inspection_status.setText('Current files checked automatically. Online lookup used the saved tags and recording identifiers.')
        self.job(work,done,label='Find missing metadata' if mode=='metadata' else 'Check front covers')

    def guide_album_fixes(self, checked=False, from_inbox=False, all_unresolved=False):
        if self.worker or self.demo_mode:return
        from .tag_review import check_album_tags
        from .repair_review import AlbumRepairReview
        sources = [] if from_inbox else self.tools_scope_rows()
        if not from_inbox and not sources:
            self.tools_status.setText('Refresh local tags, then choose the files to review.');return
        if sources:
            root = sources[0]['root']
            cached = {r['path']:r for r in sources}
            paths = list(cached)
        else:
            indices = list(self.artist_table.selectionModel().selectedRows()) if from_inbox and not all_unresolved else []
            if indices:
                artists = [self.artist_rows[source_row(self.artist_table,i.row())] for i in indices if not self.artist_table.isRowHidden(i.row())]
            else:
                resolved = {r['artist'] for r in self.store.rows("SELECT artist FROM mappings WHERE status IN ('auto','confirmed')")}
                artists = [(a,t) for a,t in self.artist_rows if a not in resolved]
            inventory = {r['path']:r['root'] for r in self.store.rows('SELECT path,root FROM local_files WHERE present=1')}
            paths = list(dict.fromkeys(t['path'] for _,tracks in artists for t in tracks if t['path'].lower().endswith('.flac') and t['path'] in inventory))
            roots = sorted({inventory[p] for p in paths})
            if not roots:
                QMessageBox.information(self,'No files to review','Select artists to review, or leave the selection empty to check unresolved artists.');return
            root = roots[0]
            if len(roots)>1:
                root,ok=QInputDialog.getItem(self,'Choose library','Review one library at a time:',roots,0,False)
                if not ok:return
            paths = [p for p in paths if inventory[p]==root]
            cached = {r['path']:r for r in self.tools_snapshots.get(root,[])}
        def work(cancel,progress):
            rows=[]
            for path in paths:
                if cancel():return []
                from .library_workflows import inspect_snapshot
                rows.extend(inspect_snapshot(root,[cached[path]] if path in cached else [],cancel,progress,self.store.preferences('organisation'),paths=[path]))
            return check_album_tags(rows,self.store,self.market,self.api(cancel,progress),cancel,progress,correct_metadata=True)
        def review(rows):
            if not rows:return
            dialog=AlbumRepairReview(rows,self)
            if dialog.exec()!=QDialog.DialogCode.Accepted or not dialog.plans:return
            self.nav.setCurrentRow(2)
            if hasattr(self, 'link_releases_root'):
                self.link_releases_root.setCurrentIndex(self.link_releases_root.findData(root))
            self.tools_root.setCurrentIndex(self.tools_root.findData(root))
            combined={r['path']:r for r in self.tools_snapshots.get(root,[])}
            combined.update({r['path']:r for r in dialog.plans})
            self.tools_plan=list(combined.values());self.tools_snapshots[root]=self.tools_plan
            if hasattr(self, 'render_link_releases'):
                self.render_link_releases()
            if hasattr(self, 'link_search'):
                self.link_search.clear()
            self.invalidate_tools_plan()
            approved={r['path'] for r in dialog.plans}
            if hasattr(self, 'link_table'):
                self.link_table.clearSelection()
                for index,row in enumerate(self.link_plan):
                    if row['path'] in approved:
                        self.link_table.selectionModel().select(self.link_table.model().index(index,0),QItemSelectionModel.SelectionFlag.Select|QItemSelectionModel.SelectionFlag.Rows)
            if hasattr(self, 'tools_table'):
                self.tools_table.clearSelection()
                for index,row in enumerate(self.tools_plan):
                    if row['path'] in approved:
                        self.tools_table.selectionModel().select(self.tools_table.model().index(index,0),QItemSelectionModel.SelectionFlag.Select|QItemSelectionModel.SelectionFlag.Rows)
            msg = f'{len(approved)} reviewed files selected. Choices saved to the database.'
            if hasattr(self, 'link_status'):
                self.link_status.setText(msg)
            if hasattr(self, 'tools_status'):
                self.tools_status.setText(msg)
        def done(rows):
            if not self.worker.isInterruptionRequested():self._after_job=lambda:review(rows)
        self.job(work,done,label='Review album fixes')

    def tools_check_album_tags(self, checked=False, unresolved=False):
        if self.worker or self.demo_mode: return
        from .tag_review import check_album_tags
        from .maintenance import first
        if unresolved:
            resolved = {r['artist'] for r in self.store.rows("SELECT artist FROM mappings WHERE status IN ('auto','confirmed')")}
            selected = [r for r in self.tools_plan if first(r.get('tags',{}),'albumartist') not in resolved]
        else:
            selected = self.tools_scope_rows()
        if not selected:
            self.tools_status.setText('Refresh local tags, then choose visible files or selected files.');return
        root=selected[0]['root'];snapshot=self.tools_snapshots.get(root,self.tools_plan);paths={r['path'] for r in selected}
        def work(cancel,progress):
            from .freshness import prepare_library
            fresh=prepare_library(self.store,root,snapshot,cancel,progress) if not snapshot or root in self._dirty_roots else copy.deepcopy(snapshot)
            from .maintenance import first
            self.rematch_changed_artists([first(r.get('tags',{}),'albumartist') for r in fresh if r['path'] in paths and r.get('needs_artist_match')],cancel,progress)
            checked=check_album_tags([r for r in fresh if r['path'] in paths],self.store,self.market,self.api(cancel,progress),cancel,progress,correct_metadata=True)
            updates={r['path']:r for r in checked}
            return [updates.get(r['path'],r) for r in fresh]
        def done(rows):
            self.tools_plan=rows;self.tools_snapshots[root]=rows;self.invalidate_tools_plan()
            self.tools_inspection_status.setText('Local changes refreshed and recording/release links checked against online source.')
        self.job(work,done,label='Refresh files and verify Online links')

    def _tools_table_context_menu(self, pos):
        if self._select_context_row(self.tools_table,pos)<0:return
        selected = self.tools_selected()
        if not selected: return
        menu = QMenu(self)
        if any(r.get('catalogue_options') for r in selected):
            act_choose = menu.addAction('Choose online match…')
            act_choose.triggered.connect(self.tools_choose_album)
        act_inspect = menu.addAction('Inspect all tags…')
        act_inspect.triggered.connect(self.tools_inspect_tags)
        path_to_show = selected[0].get('path') if selected else None
        if path_to_show:
            menu.addSeparator()
            finder_label = 'Show in Finder' if sys.platform == 'darwin' else 'Show in file manager'
            act_finder = menu.addAction(finder_label)
            act_finder.triggered.connect(lambda: reveal_in_file_manager(path_to_show))

            ignored_paths = self.store.ignored_local_files()
            all_ignored = all(r['path'] in ignored_paths for r in selected)
            if all_ignored:
                act_unignore = menu.addAction(f"Unignore {len(selected)} track(s)" if len(selected) > 1 else "Unignore track")
                act_unignore.triggered.connect(self.tools_unignore_selected)
            else:
                act_ignore = menu.addAction(f"Ignore {len(selected)} track(s)" if len(selected) > 1 else "Ignore track")
                act_ignore.triggered.connect(self.tools_ignore_selected)
        menu.exec(self.tools_table.viewport().mapToGlobal(pos))

    def tools_ignore_selected(self):
        selected = self.tools_selected()
        if not selected: return
        for r in selected:
            self.store.ignore_local_file(r['path'])
        self.log(f"Ignored {len(selected)} track(s).")
        self.invalidate_tools_plan()
        self.refresh()

    def tools_unignore_selected(self):
        selected = self.tools_selected()
        if not selected: return
        for r in selected:
            self.store.unignore_local_file(r['path'])
        self.log(f"Unignored {len(selected)} track(s).")
        self.invalidate_tools_plan()
        self.refresh()

    def tools_remove_superseded(self):
        superseded_rows = [r for r in self.tools_plan if r.get('superseded_by')]
        if not superseded_rows:
            QMessageBox.information(self, 'No superseded singles', 'No redundant standalone singles found in the inspected files.')
            return
        lines = []
        for r in superseded_rows[:12]:
            info = r['superseded_by']
            lines.append(f"• {Path(r['path']).name} (Single Track {info['single_track']})\n   ↳ Exists as Track {info['album_track']} on '{info['album_title']}'")
        if len(superseded_rows) > 12:
            lines.append(f"... and {len(superseded_rows) - 12} more.")

        msg = (f"Found {len(superseded_rows)} standalone single track(s) that already exist within your full EP/Album releases:\n\n"
               + "\n".join(lines)
               + "\n\nMove these redundant single file(s) to Trash? The full EP/Album releases will be kept.")
        ans = QMessageBox.question(self, 'Remove Superseded Singles', msg, QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No)
        if ans == QMessageBox.StandardButton.Yes:
            def work(cancel, progress):
                from .maintenance import trash_file
                removed_count = 0
                trashed_set = set()
                with self.store.connect() as db:
                    for i, r in enumerate(superseded_rows):
                        if cancel():
                            break
                        progress(f"Moving to Trash ({i+1}/{len(superseded_rows)}) · {Path(r['path']).name}")
                        try:
                            trash_file(r['path'])
                            db.execute('UPDATE local_files SET present=0 WHERE path=?', (r['path'],))
                            removed_count += 1
                            trashed_set.add(r['path'])
                        except Exception as exc:
                            progress(f"Could not move {Path(r['path']).name} to Trash · {exc}")
                return f"Moved {removed_count} superseded single track(s) to Trash.", trashed_set

            def done(result):
                msg, trashed_set = result
                self.log(msg)
                self.tools_plan = [r for r in self.tools_plan if r['path'] not in trashed_set]
                if hasattr(self, 'tools_root') and self.tools_root.currentData():
                    self.tools_snapshots[self.tools_root.currentData()] = self.tools_plan
                self.invalidate_tools_plan()
                self.refresh()

            self.job(work, done, label="Remove superseded singles", local=True, is_disk_op=True)

    def tools_choose_album(self):
        if self.worker: return
        is_link_page = hasattr(self, 'nav') and self.nav.currentRow() == 2
        selected = self.link_selected() if is_link_page else self.tools_selected()
        if not selected:
            status_lbl = self.link_status if is_link_page else self.tools_status
            status_lbl.setText('Select a file to choose between its verified album placements.');return
        row = selected[0];options = row.get('catalogue_options',[])
        if not options:
            status_lbl = self.link_status if is_link_page else self.tools_status
            status_lbl.setText('No alternative verified placements saved for this file. Verify tags with online catalogue first.');return

        dialog = OnlineAlbumLinkDialog(self, row, options)
        if dialog.exec() and dialog.chosen_option:
            chosen = dialog.chosen_option
            from .linking import save_result
            p_dir = Path(row['path']).parent
            affected = [r for r in selected if Path(r['path']).parent == p_dir and r.get('catalogue_options')]
            if not affected: affected = [row]
            saved_count=0
            for r in affected:
                # A sibling needs its own recording placement. Never reuse the
                # first file's track ID when this edition is absent for it.
                matches=[o for o in r.get('catalogue_options',[]) if str(o['id'])==str(chosen['id'])]
                matching_opt=next((o for o in matches if o.get('artist')==chosen.get('artist')),matches[0] if matches else None)
                if matching_opt is None:continue
                updated=dict(r,catalogue_choice=matching_opt,catalogue_note=matching_opt.get('evidence','')+' · placement chosen by you')
                if save_result(self.store,self.market,updated,manual=True):r.update(updated);saved_count+=1
            self.invalidate_tools_plan()
            if hasattr(self, 'render_link_releases'):
                self.render_link_releases()
            msg = f"{saved_count} links saved · {len(affected)-saved_count} unchanged. Local files unchanged."
            if is_link_page: self.link_status.setText(msg)
            else: self.tools_status.setText(msg)

    def choose_tidal_placement(self):
        return self.tools_choose_album()

    def tools_set_artist(self):
        self.tools_set_artist_field('albumartist')

    def tools_set_artist_field(self,field):
        if self.worker or self.tools_operation() not in ('tags','links'):return
        selected=self.tools_scope_rows()
        if not selected:return
        label='Album artist (library grouping)' if field=='albumartist' else 'Track artist (performer credits)'
        value,ok=QInputDialog.getText(self,label,f'Set {label.lower()} for {len(selected):,} files. Other artist fields stay unchanged:')
        if ok and value.strip():
            for row in selected:
                if row.get('tags'):row.setdefault('overrides',{})[field]=[value.strip()]
            self.invalidate_tools_plan()

    def tools_set_date(self):
        if self.worker or self.tools_operation() not in ('tags','links'):return
        selected=self.tools_scope_rows()
        if not selected:return
        value,ok=QInputDialog.getText(self,'Release date',f'Date for {len(selected):,} files: YYYY, YYYY-MM or YYYY-MM-DD. Original-date tags stay unchanged.')
        if not ok:return
        cleaned=normalized_date(value)
        if not cleaned:
            QMessageBox.warning(self,'Invalid date','Enter a valid year, month or full date.');return
        for row in selected:
            if row.get('tags'):row.setdefault('overrides',{})['date']=[cleaned]
        self.invalidate_tools_plan()

    def tools_inspect_tags(self):
        selected=self.tools_selected()
        if not selected:return
        dialog=QDialog(self);dialog.setWindowTitle('FLAC tags · '+Path(selected[0]['path']).name);dialog.resize(720,520)
        layout=QVBoxLayout(dialog);view=QPlainTextEdit();view.setReadOnly(True)
        size=selected[0].get('cover_size')
        layout.addWidget(QLabel(f'Embedded front cover: {size[0]} × {size[1]}' if size else 'No readable embedded front cover'))
        if selected[0].get('artwork_change'):layout.addWidget(QLabel('Proposed front cover: 1280 × 1280'))
        view.setPlainText(json.dumps(selected[0].get('tags',{}),ensure_ascii=False,indent=2));layout.addWidget(view)
        dialog.exec()

    def tools_select_changes(self):
        ignored_paths = self.store.ignored_local_files()
        select_rows(self.tools_table,(index for index,row in enumerate(self.tools_plan)
            if not self.tools_table.isRowHidden(index) and row['path'] not in ignored_paths
            and (row.get('changes') or row.get('artwork_change') or row['path']!=row['target'] or row.get('superseded_by'))
            and not (row.get('blocked') or row.get('collision') or row.get('result'))))

    def tools_apply(self):
        if self.worker:return
        if self.demo_mode:return self.demo_notice()
        mode=self.tools_operation()
        if mode == 'summary':
            QMessageBox.information(
                self,
                'Select a Tool Tab',
                'The Summary tab displays an overview of all library issues.\n\n'
                'To review and apply changes, please switch to a specific tool tab above (e.g. "Fix existing tags", "Add missing tags", "Add missing covers", or "Organise files & folders").'
            )
            return
        from .library_workflows import has_changes,validate_operation
        ignored_paths = self.store.ignored_local_files()
        selected=[r for r in self.tools_selected() if has_changes(r) and not r.get('blocked') and not r.get('collision') and r['path'] not in ignored_paths]
        if not selected:return
        try:
            for row in selected:validate_operation(row,mode)
        except ValueError as exc:QMessageBox.warning(self,'Refresh this plan',str(exc));return
        labels={'tags':'Update tags','metadata':'Add missing tags','artwork':'Apply front covers','organise':'Organise files','links':'Apply online tag corrections'}

        tags_count = sum(bool(r.get('changes')) for r in selected)
        trash_count = sum(bool(r.get('superseded_by')) for r in selected)
        move_count = sum(r.get('target') != r.get('path') and not r.get('superseded_by') for r in selected)

        if mode == 'tags':
            msg = (f"Update tags in {len(selected):,} file(s)?\n\n"
                   f"• Only selected local tag cleanups will be written.\n"
                   f"• Files stay in their current folders. No files are moved or deleted.\n"
                   f"• No online lookups will run.\n\n"
                   "Review the exact changes in the table before continuing.")
            if QMessageBox.question(self, 'Update tags', msg) != QMessageBox.StandardButton.Yes:
                return
        elif mode == 'organise':
            details = []
            if move_count: details.append(f"{move_count:,} file(s) to move/rename to match layout")
            if trash_count: details.append(f"{trash_count:,} redundant single(s) to move to Trash")
            summary = " (" + ", ".join(details) + ")" if details else ""
            msg = (f"Organise {len(selected):,} file(s){summary}?\n\n"
                   + (f"• {move_count:,} file(s) will be moved/renamed to match your folder/filename layout.\n" if move_count else "")
                   + (f"• {trash_count:,} redundant single file(s) will be moved to system Trash (full releases are kept).\n" if trash_count else "")
                   + "• Audio tags remain unchanged (unless reuniting stray tracks).\n"
                   + "• No online lookups will run.\n\n"
                   "Review the exact changes in the table before continuing.")
            if QMessageBox.question(self, 'Organise files', msg) != QMessageBox.StandardButton.Yes:
                return
        else:
            effects={'links':'Only the displayed online tag corrections will be written. Files stay in their current folders. Existing BPM and key are protected.',
                'metadata':'Only the displayed missing tags will be added. Existing values and file locations stay unchanged.',
                'artwork':'Only the reviewed front covers will change. Tags, audio and file locations stay unchanged.'}
            if QMessageBox.question(self,labels[mode],f'{labels[mode]} for {len(selected):,} files?\n\n{effects.get(mode, "")}\n\nReview the exact changes in the table before continuing. No backup copies are kept.')!=QMessageBox.StandardButton.Yes:return
        def done(result):
            message,updates=result
            trashed_paths = {r['path'] for r in selected if r.get('superseded_by') and updates.get(r['path'], {}).get('result') == 'applied'}
            self.tools_plan=[updates.get(row['path'],row) for row in self.tools_plan if row['path'] not in trashed_paths]
            self.tools_snapshots[self.tools_root.currentData()]=self.tools_plan
            self.invalidate_tools_plan();self.tools_status.setText(message+' · Current tags, Artists and library counts updated.')
            self.tools_inspection_status.setText('Changes saved. The next Online check uses the updated tags automatically.')
            self.artist_filter.setCurrentText('All artists')
            if self._link_enabled:self._link_restart=True
            self._library_content_changed(self.tools_root.currentData())
        def work(cancel,progress):
            import copy
            plans=copy.deepcopy(selected)
            message=apply_plans(plans,self.store,cancel,progress)
            updates={}
            for row in plans:
                original=row['path']
                if row.get('result')=='applied':
                    if row.get('superseded_by'):
                        updates[original]={'path': original, 'result': 'applied', 'blocked': 'Moved to Trash'}
                        continue
                    fresh=inspect_file(row['target'],row['root'],row.get('layout'));updates[original]=fresh
                    from .linking import retain_after_apply,invalidate_related_links
                    retain_after_apply(self.store,self.market,row,fresh)
                    identity_tags={'albumartist','artist','album','title','isrc','tracknumber','track','discnumber','tracktotal','totaltracks','disctotal','totaldiscs'}
                    if mode in ('tags','links') and identity_tags.intersection(row.get('changes',{})):
                        invalidate_related_links(self.store,self.market,row,fresh)
                    if mode in ('tags','links') and any(k in row['changes'] for k in ('albumartist','album')):fresh['needs_artist_match']=True
                elif row.get('result'):row['apply_error']=row['result'];updates[original]=row
            return message,updates
        self.job(work,done,label=labels[mode],local=True,is_disk_op=True)

    def set_job_actions_busy(self,busy):
        if busy:
            if getattr(self,'_job_button_states',[]):return
            self._job_button_states=[]
            for button in self.findChildren(QPushButton):
                if button.text().startswith(('Recheck','Start / resume linking','Scan for new releases','Check availability','Find missing tags','Find 1280','Refresh release list','Test configured connection','Inspect library','Find optimizations','Check candidate releases')):
                    self._job_button_states.append((button,button.isEnabled()));button.setEnabled(False)
        else:
            for button,enabled in getattr(self,'_job_button_states',[]):button.setEnabled(enabled)
            self._job_button_states=[]

    def job(self, operation, completed=None, label='Library operation', on_failure=None, local=False, is_disk_op=False):
        if is_disk_op and self._preview_worker:
            self._after_preview_job=(operation,completed,label,on_failure,local,is_disk_op)
            self.log('Waiting for library inspection before changing files…');return
        if self._link_worker and self._link_worker.isRunning() and not local:
            # Never wait on a network worker from the GUI thread.  Stop it
            # cooperatively and start this job from linking_finished instead.
            self._link_worker.requestInterruption()
            self.set_job_actions_busy(True)
            self._deferred_job=((operation,completed,label,on_failure,local,is_disk_op),{})
            self._resume_bg_linking_after_job = True
            self.log(f'Queued · {label} · waiting for background linking to stop')
            return
        if self.worker:
            QMessageBox.information(self, 'Job in progress', 'Wait for the current job or cancel it first.'); return
        self._is_background_job = False
        self._is_disk_operation = bool(is_disk_op)
        if hasattr(self, 'tools_apply_button'):
            self.tools_apply_button.setEnabled(False)
        self.job_label = label; self.job_started = time.monotonic(); self.job_failed = False
        self.worker = Worker(operation)
        self.set_job_actions_busy(True)
        self.worker.message.connect(self.log)
        self._job_completed=completed;self._job_on_failure=on_failure
        self.worker.failure.connect(self.job_failure,Qt.ConnectionType.QueuedConnection)
        self.worker.result.connect(self.job_result,Qt.ConnectionType.QueuedConnection)
        self.worker.finished.connect(self.job_finished,Qt.ConnectionType.QueuedConnection)
        self.cancel.setEnabled(True); self.busy.show()
        if hasattr(self, 'activity_job_icon'):
            self.activity_job_icon.setText('🔄')
            self.activity_job_heading.setText(f'Running: {label}')
            self.activity_job_detail.setText('Working…')
            self.activity_cancel_btn.setEnabled(True)
        if hasattr(self, 'activity_nav_btn'):
            self.activity_nav_btn.setText('Activity (Running…)')
        if hasattr(self, 'nav') and self.nav.item(8):
            self.nav.item(8).setText('⚡ Activity (Running…)')
        self.log(f'Started · {label} · open Activity for detailed steps')
        self.job_timer.start(); self.update_elapsed(); self.worker.start()

    @Slot(str)
    def job_failure(self,message):
        self.job_failed=True
        self._is_disk_operation = False
        if self._job_on_failure:self._job_on_failure(message)
        self.log(f'{self.job_label} · {message}')
        # A nested modal loop here can deliver finished while result handling is active.
        self.tools_status.setText(message)

    @Slot(object)
    def job_result(self,result):
        try:
            if isinstance(result,str):self.log(result)
            elif isinstance(result,dict) and 'status' in result:
                self.log(f"Scan {result['status']} · {result.get('read',0)} read · {result.get('unchanged',0)} unchanged · {result.get('missing',0)} removed from index")
            if self._job_completed:self._job_completed(result)
        except Exception as exc:
            from .diagnostics import record_failure
            record_failure('Displaying job results',exc)
            self.job_failure('Could not display the completed results. Refresh the library; diagnostic details were saved.')

    @Slot()
    def job_finished(self):
        worker = self.worker; self.worker = None
        self.set_job_actions_busy(False)
        pending=getattr(self,'_pending_disclosure',None);self._pending_disclosure=None
        if pending:QTimer.singleShot(0,lambda:self.expand_release_id(pending))
        self._is_disk_operation = False
        self.job_timer.stop(); self.busy.hide(); self.cancel.setEnabled(False)
        self.queue_page_widget.setEnabled(True)
        self.tools_page_widget.setEnabled(True)
        if hasattr(self, 'activity_job_icon'):
            if self._link_worker and self._link_worker.isRunning():
                self.activity_job_icon.setText('🔄')
                self.activity_job_heading.setText('Running: Linking releases and tracks in background')
                self.activity_job_detail.setText('Querying Online catalogue…')
                if hasattr(self, 'activity_pause_link_btn'):
                    self.activity_pause_link_btn.setText('Pause background linking')
                    self.activity_pause_link_btn.setVisible(True)
                    self.activity_pause_link_btn.setEnabled(True)
            else:
                self.activity_job_icon.setText('⚡')
                self.activity_job_heading.setText('No background tasks currently running')
                self.activity_job_detail.setText('Ready for operations.')
                if hasattr(self, 'activity_pause_link_btn'):
                    self.activity_pause_link_btn.setVisible(False)
                if hasattr(self, 'activity_job_eta'):
                    self.activity_job_eta.setVisible(False)
            self.activity_cancel_btn.setEnabled(False)
        if hasattr(self, 'activity_nav_btn'):
            self.activity_nav_btn.setText('Activity')
        if hasattr(self, 'nav') and self.nav.item(8):
            self.nav.item(8).setText('📋  Activity')
        elapsed = time.monotonic() - self.job_started
        ending = 'Stopped with an error or cancellation' if self.job_failed else ('Cancelled' if worker.isInterruptionRequested() else 'Finished')
        self.log(f'{ending} · {self.job_label} · {elapsed:.1f} seconds')
        if worker:
            worker.wait(2000)
            worker.deleteLater()
        try:
            self.refresh()
            self.update_tools_selection()
        except Exception:
            pass
        after = getattr(self, '_after_job', None); self._after_job = None
        if after and not self.job_failed and ending!='Cancelled': QTimer.singleShot(0, after)
        self.resume_changed_links()
        if getattr(self, '_resume_bg_linking_after_job', False):
            self._resume_bg_linking_after_job = False
            QTimer.singleShot(1500, self._check_background_linking)

    def refresh(self):
        if self._view_worker:self._view_pending=True;return
        try:
            active_artists = set(self.store.artists(include_compilations=True).keys())
            if active_artists:
                placeholders = ','.join('?' for _ in active_artists)
                with self.store.connect() as db:
                    db.execute(f"DELETE FROM match_reviews WHERE artist NOT IN ({placeholders})", list(active_artists))
                    db.execute(f"DELETE FROM mappings WHERE manual=0 AND status != 'confirmed' AND artist NOT IN ({placeholders})", list(active_artists))
        except Exception:
            pass
        try:
            size=self.store.rows('SELECT COUNT(*) AS n FROM local_files WHERE present=1')[0]['n']
        except Exception:
            return
        if size<200:
            self._view_ready(build_view(self.store,self.market));return
        self._view_worker=Worker(lambda cancel,progress:build_view(self.store,self.market))
        self._view_worker.result.connect(self._view_ready,Qt.ConnectionType.QueuedConnection)
        self._view_worker.failure.connect(self.log,Qt.ConnectionType.QueuedConnection)
        self._view_worker.finished.connect(self._view_finished,Qt.ConnectionType.QueuedConnection)
        self._view_worker.start()

    @Slot()
    def _view_finished(self):
        worker=self._view_worker;self._view_worker=None
        if worker:
            worker.wait(2000)
            worker.deleteLater()
        if self._view_pending and not self._closing_requested:self._view_pending=False;self.refresh()

    def get_checked_roots(self):
        if hasattr(self, 'active_drive_combo') and self.active_drive_combo.currentData():
            return [self.active_drive_combo.currentData()]
        if hasattr(self, 'roots') and hasattr(self, 'root_rows'):
            row = source_row(self.roots,self.roots.currentRow())
            if 0 <= row < len(self.root_rows):
                return [self.root_rows[row]['root']]
            if self.root_rows:
                return [self.root_rows[0]['root']]
        return []

    @Slot(object)
    def _view_ready(self,data):
        if self._closing_requested or self._view_pending:return
        self._view_data=data
        self.update_category_cards(data)
        roots = data['roots']
        fill(self.roots, [(r['root'], format_user_datetime(r['scanned_at']) if r.get('scanned_at') else 'Never', r['status']) for r in roots])
        if hasattr(self, 'overview_roots'): fill(self.overview_roots, [(r['root'], format_user_datetime(r['scanned_at']) if r.get('scanned_at') else 'Never', r['status']) for r in roots])
        for widget in (self.roots, self.overview_roots):
            for row in range(widget.rowCount()):
                item = widget.item(row, 0)
                if item: item.setFlags(item.flags() & ~Qt.ItemFlag.ItemIsUserCheckable)
        self.root_rows = roots
        selected_root = getattr(self, 'active_drive_combo', None) and self.active_drive_combo.currentData()
        if not selected_root and hasattr(self, 'tools_root'):
            selected_root = self.tools_root.currentData()
        self.tools_root.blockSignals(True);self.tools_root.clear()
        for root in roots:self.tools_root.addItem(root['root'],root['root'])
        if selected_root is not None:self.tools_root.setCurrentIndex(max(0,self.tools_root.findData(selected_root)))
        self.tools_root.blockSignals(False)
        if hasattr(self, 'link_releases_root'):
            self.link_releases_root.blockSignals(True)
            self.link_releases_root.clear()
            for root in roots: self.link_releases_root.addItem(root['root'], root['root'])
            if selected_root is not None: self.link_releases_root.setCurrentIndex(max(0, self.link_releases_root.findData(selected_root)))
            self.link_releases_root.blockSignals(False)
        if hasattr(self, 'active_drive_combo'):
            self.active_drive_combo.blockSignals(True)
            self.active_drive_combo.clear()
            for root in roots: self.active_drive_combo.addItem(root['root'], root['root'])
            if selected_root is not None: self.active_drive_combo.setCurrentIndex(max(0, self.active_drive_combo.findData(selected_root)))
            self.active_drive_combo.blockSignals(False)
            if self.active_drive_combo.currentIndex() >= 0 and self.roots.rowCount() > self.active_drive_combo.currentIndex():
                self.roots.blockSignals(True)
                self.roots.selectRow(next(n for n in range(self.roots.rowCount()) if source_row(self.roots,n)==self.active_drive_combo.currentIndex()))
                self.roots.blockSignals(False)
        self.artist_rows = list(data['artists'].items())
        mappings=data['mappings'];reviews=data['reviews'];self._catalogue_types=data['types']
        artist_display=[]
        for name,tracks in self.artist_rows:
            state,evidence=self.artist_state(name,mappings.get(name,{}),reviews.get(name,{}))
            releases = self.local_releases(name, tracks)
            unknown = any(r[2] == 'Unknown' for r in releases)
            singles = sum(r[2] in ('SINGLE', 'Single') for r in releases)
            albums = sum(r[2] in ('ALBUM', 'Album', 'EP') for r in releases)
            suffix = ' + ?' if unknown else ''
            artist_display.append((name,len(tracks),str(singles)+suffix,str(albums)+suffix,self.match_label(state),evidence))
        fill(self.artist_table, artist_display)
        self.filter_artists()
        activity_rows = []
        for r in data.get('scans', []):
            started = r.get('started')
            dt = parse_utc_or_iso(started)
            date_str = dt.strftime('%Y-%m-%d') if dt else (str(started) or '—')
            time_str = dt.strftime('%H:%M:%S') if dt else '—'
            activity_rows.append((date_str, time_str, r.get('status', '—'), self.scan_summary(r.get('summary', ''))))
        fill(self.activity, activity_rows)
        count = data.get('track_count', 0); unresolved = sum(1 for n, _ in self.artist_rows if mappings.get(n, {}).get('status') not in ('confirmed', 'auto'))
        total_artists = len(self.artist_rows)
        resolved_artists = max(0, total_artists - unresolved)
        queued = sum(r.get('decision')=='queued' for r in data.get('queue', []))
        from .link_statistics import link_statistics
        stats=data.get('link_statistics') or link_statistics(self.store,self.market)
        count=stats['track_count'];release_count=stats['release_count']
        linked=stats['linked_tracks'];resolved_releases=stats['linked_releases']

        track_pct = round(100 * linked / count) if count else 0
        artist_pct = round(100 * resolved_artists / total_artists) if total_artists else 0
        release_pct = round(100 * resolved_releases / release_count) if release_count else 0

        if hasattr(self, 'card_tracks_metric'):
            self.card_tracks_metric.setText(f'{linked:,} / {count:,} resolved')
            self.card_tracks_sub.setText(f'{track_pct}% linked to online recordings' if count else 'No tracks scanned yet')

        if hasattr(self, 'card_artists_metric'):
            self.card_artists_metric.setText(f'{resolved_artists:,} / {total_artists:,} resolved')
            self.card_artists_sub.setText(f'{artist_pct}% matched with Online profiles' if total_artists else 'No artists found')

        if hasattr(self, 'card_releases_metric'):
            self.card_releases_metric.setText(f'{resolved_releases:,} / {release_count:,} resolved')
            self.card_releases_sub.setText(f'{release_pct}% with verified album IDs' if release_count else 'No releases found')

        if hasattr(self, 'card_queue_metric'):
            self.card_queue_metric.setText(f'{queued:,} queued' if queued else '0 queued')
            self.card_queue_sub.setText(f'{queued:,} releases ready to download' if queued else 'No pending downloads')

        self._update_card_favs_metric()

        if hasattr(self, 'card_hygiene_metric'):
            ignored_paths = self.store.ignored_local_files()
            needs_fix_count = 0
            unignored_count = 0
            if getattr(self, 'tools_plan', None):
                from .maintenance import date_changes, disc_changes, track_changes, stray_changes
                from .musical_keys import key_changes
                from .library_workflows import LYRIC_TAGS
                strays = stray_changes(self.tools_plan)
                for r in self.tools_plan:
                    p = r.get('path')
                    if p in ignored_paths:
                        continue
                    unignored_count += 1
                    tags = r.get('tags', {})
                    if (date_changes(tags)[0] or disc_changes(tags)[0] or track_changes(tags)[0] or key_changes(tags)[0] or
                        any(k in tags for k in LYRIC_TAGS) or p in strays or r.get('issues') or r.get('superseded_by')):
                        needs_fix_count += 1
            else:
                for tracks in data['artists'].values():
                    for t in tracks:
                        p = t.get('path')
                        if p in ignored_paths:
                            continue
                        unignored_count += 1
                        d = t.get('date', '')
                        k = t.get('musical_key')
                        disc = t.get('disc')
                        needs_disc_pad = False
                        if disc is not None and str(disc).isdigit() and len(str(disc)) < 2:
                            needs_disc_pad = True
                        if (not t.get('artist') or not t.get('album') or not t.get('title') or not t.get('track') or
                            (d and normalized_date(d) != d) or needs_disc_pad or
                            (k and camelot_key(k) and camelot_key(k) != k)):
                            needs_fix_count += 1
            hygiene_total = unignored_count if unignored_count else count
            fix_pct = round(100 * needs_fix_count / hygiene_total) if hygiene_total else 0
            if not hygiene_total:
                self.card_hygiene_metric.setText('—')
                self.card_hygiene_sub.setText('No local files scanned')
            elif needs_fix_count > 0:
                self.card_hygiene_metric.setText(f'{fix_pct}% require fixes')
                self.card_hygiene_sub.setText(f'{needs_fix_count:,} of {hygiene_total:,} local files need tag cleanups')
            else:
                self.card_hygiene_metric.setText('100% clean')
                self.card_hygiene_sub.setText('All dates, discs and keys standardised')

        for label,text in zip(self.metrics,[f'{count:,}\nLocal tracks',f'{len(self.artist_rows):,}\nAlbum artists',f'{release_count:,}\nReleases',f'{unresolved:,}\nArtists to resolve','—\nAvailable releases',f'{queued:,}\nIn download queue']):label.setText(text)
        self.metrics[2].setToolTip('Distinct local album/single/EP titles per album artist.')
        self.metrics[4].setToolTip('Verified available missing releases across all dates, excluding items already queued or ignored. Based on saved catalogues.')
        self.refresh_coverage(); self.refresh_queue(); self.refresh_overview_tables(); self.refresh_downloaded_releases()

    def refresh_overview_tables(self):
        if not hasattr(self, 'overview_missing') or not hasattr(self, 'overview_downloaded'):
            return
        # 1. Latest missing releases (top 10)
        missing_rows = []
        base_coverage = getattr(self, '_base_coverage_rows', [])
        for rel_item in base_coverage:
            state = rel_item.get('state', '')
            if 'missing' in state.casefold() or 'partial' in state.casefold():
                rel = rel_item.get('release', {})
                artist = rel.get('artist') or 'Unknown'
                title = rel.get('title') or 'Unknown'
                date = rel.get('date') or '—'
                rel_type = format_release_type(rel.get('type') or 'Album')
                track_count = rel.get('track_count') if rel.get('track_count') is not None else (len(rel.get('tracks', [])) if rel.get('tracks_loaded') else '—')
                missing_rows.append((artist, title, str(date), str(rel_type), str(track_count)))
                if len(missing_rows) >= 10:
                    break
        fill(self.overview_missing, missing_rows)

        # 2. Latest downloaded releases (top 10)
        downloaded_rows = []
        try:
            q_rows = self.store.rows("SELECT * FROM queue WHERE decision='downloaded' ORDER BY updated DESC LIMIT 10")
            for r in q_rows:
                payload = json.loads(r['payload']) if isinstance(r['payload'], str) else r['payload']
                artist = payload.get('artist') or 'Unknown'
                title = payload.get('title') or 'Unknown'
                updated_date = (r.get('updated') or '')[:10] or '—'
                rel_type = format_release_type(payload.get('type') or 'Album')
                track_count = str(payload.get('track_count', len(payload.get('tracks', [])) or '—'))
                downloaded_rows.append((artist, title, updated_date, rel_type, track_count))
        except Exception:
            pass
        fill(self.overview_downloaded, downloaded_rows)

    def selected_artist(self):
        row = source_row(self.artist_table,self.artist_table.currentRow())
        return self.artist_rows[row] if 0 <= row < len(self.artist_rows) else None

    def local_releases(self, name, tracks):
        types = getattr(self,'_catalogue_types',{}).get(name,{})
        grouped = {}
        grouped_tracks = {}
        for track in tracks:
            key = title_key(track.get('album', ''))
            row = grouped.setdefault(key, [track.get('album') or 'Unknown release', 0, format_release_type(types.get(key, 'Unknown')), track.get('date', '')])
            row[1] += 1
            grouped_tracks.setdefault(key, []).append(track)
        self._current_local_release_tracks = list(grouped_tracks.values())
        return list(grouped.values())

    def _local_table_context_menu(self, pos):
        row=self._select_context_row(self.local_table,pos)
        tracks_list = getattr(self, '_current_local_release_tracks', [])
        if 0 <= row < len(tracks_list):
            tracks = tracks_list[source_row(self.local_table,row)]
            if tracks and tracks[0].get('path'):
                menu = QMenu(self)
                finder_label = 'Show in Finder' if sys.platform == 'darwin' else 'Show in file manager'
                act_finder = menu.addAction(finder_label)
                act_finder.triggered.connect(lambda: reveal_in_file_manager(tracks[0]['path']))
                act_ext = menu.addAction('Extended review for this release…')
                act_ext.triggered.connect(lambda: self.extended_artist_review())
                menu.exec(self.local_table.viewport().mapToGlobal(pos))

    def _coverage_table_context_menu(self, pos):
        # coverage_table is a model/view VirtualTable, not a QTableWidget.
        # Resolve the viewport coordinate through QTableView and reject clicks in
        # empty space before looking up the backing release row.
        index = self.coverage_table.indexAt(pos)
        if not index.isValid():
            return
        self._select_context_row(self.coverage_table,pos)
        item_rel = self.selected_release()
        if not item_rel: return
        local_paths = item_rel.get('local_tracks') or []
        row = index.row()
        if 0 <= row < len(self.coverage_rows):
            cov_row = self._coverage_item(row)
            if cov_row.get('is_track') and cov_row.get('parent'):
                local_paths = cov_row['parent'].get('local_tracks') or local_paths

        from .context_actions import context_menu
        menu=context_menu(self,path=local_paths[0] if local_paths else None,metadata=item_rel,choose=lambda:self.review_release(item_rel))
        act_open = menu.addAction('Open on web')
        act_open.triggered.connect(self.open_release)
        act_check = menu.addAction('Recheck release availability')
        act_check.triggered.connect(lambda:self.check_release_links(force_selected=True))
        act_tracks = menu.addAction('Load or recheck track details')
        act_tracks.triggered.connect(self.check_release_tracks)
        menu.exec(self.coverage_table.viewport().mapToGlobal(pos))

    def _artist_table_context_menu(self, pos):
        row=self._select_context_row(self.artist_table,pos)
        if not (0 <= row < len(self.artist_rows)):
            return
        artist_name, tracks = self.artist_rows[source_row(self.artist_table,row)]
        menu = QMenu(self)

        mapping = self._view_data.get('mappings', {}).get(artist_name, {}) if getattr(self, '_view_data', None) else {}
        tidal_id = mapping.get('tidal_id')
        status = mapping.get('status')
        if not tidal_id:
            m_rows = self.store.rows('SELECT tidal_id, status FROM mappings WHERE artist=? AND tidal_id IS NOT NULL', (artist_name,))
            if m_rows:
                tidal_id = m_rows[0].get('tidal_id')
                status = m_rows[0].get('status')

        if tidal_id:
            act_open = menu.addAction('Open on web')
            act_open.triggered.connect(lambda: QDesktopServices.openUrl(QUrl(f'https://tidal.com/browse/artist/{tidal_id}')))

        if tracks and tracks[0].get('path'):
            finder_label = 'Reveal artist folder in Finder' if sys.platform == 'darwin' else 'Reveal artist folder'
            act_finder = menu.addAction(finder_label)
            act_finder.triggered.connect(lambda: reveal_in_file_manager(str(Path(tracks[0]['path']).parent)))

        menu.addSeparator()

        act_review = menu.addAction('Review matches…')
        act_review.triggered.connect(self.review_candidates)

        act_ext = menu.addAction('Extended review (deep scan)…')
        act_ext.triggered.connect(lambda: self.extended_artist_review(artist_name))

        if tidal_id or status in ('confirmed', 'auto'):
            menu.addSeparator()
            act_unlink = menu.addAction('Unlink artist')
            act_unlink.triggered.connect(self.unlink)

        menu.exec(self.artist_table.viewport().mapToGlobal(pos))

    def _queue_table_context_menu(self, pos):
        row=self._select_context_row(self.queue_table,pos)
        displayed = getattr(self, 'displayed_queue_rows', [])
        if not (0 <= row < len(displayed)):
            return
        entry = displayed[row]
        from .context_actions import context_menu
        metadata=entry.get('payload') or entry.get('track') or entry.get('record')
        destination=(entry.get('payload') or {}).get('existing_destination') or {}
        local_path=str(Path(destination['root'])/destination['album_relative']) if destination.get('root') and destination.get('album_relative') else None
        menu=context_menu(self,path=local_path,metadata=metadata)

        rel_id = entry.get('id') if entry.get('type') == 'release' else entry.get('parent_id')
        track_id = entry.get('track', {}).get('id') if entry.get('type') == 'track' else None

        if track_id:
            act_open = menu.addAction('Open on web')
            act_open.triggered.connect(lambda: QDesktopServices.openUrl(QUrl(f'https://tidal.com/browse/track/{track_id}')))
        elif rel_id:
            act_open = menu.addAction('Open on web')
            act_open.triggered.connect(lambda: QDesktopServices.openUrl(QUrl(f'https://tidal.com/browse/album/{rel_id}')))

        menu.addSeparator()
        act_remove = menu.addAction('Remove from Queue')
        act_remove.triggered.connect(self.remove_queue)

        menu.exec(self.queue_table.viewport().mapToGlobal(pos))

    def local_details(self):
        selected = self.selected_artist()
        fill(self.local_table, self.local_releases(*selected) if selected else [])
        if hasattr(self, 'btn_match_sel'):
            has_sel = bool(self.artist_table.selectionModel() and self.artist_table.selectionModel().selectedRows())
            self.btn_match_sel.setEnabled(has_sel)

    def add_library(self):
        if self.demo_mode: return self.demo_notice()
        root = QFileDialog.getExistingDirectory(self, 'Choose a music library')
        if root: self.job(lambda cancel, progress: scan(self.store, root, cancelled=cancel, progress=progress), label=f'Scan library · {root}',local=True)

    def rescan(self, checked=False, force=False):
        if self.demo_mode: return self.demo_notice()
        active_root = self.active_drive_combo.currentData() if hasattr(self, 'active_drive_combo') else None
        if not active_root and hasattr(self, 'roots') and self.roots.currentRow() >= 0 and self.roots.currentRow() < len(self.root_rows):
            active_root = self.root_rows[source_row(self.roots,self.roots.currentRow())]['root']
        if not active_root and getattr(self, 'root_rows', None):
            active_root = self.root_rows[0]['root']
        target_roots = [active_root] if active_root else []
        if target_roots:
            def work(cancel, progress):
                from .library_workflows import inspect_snapshot
                last_result = None
                for root in target_roots:
                    if cancel(): break
                    progress(f'Scanning library ({root})…')
                    snapshot = self.tools_plan if self.tools_plan and self.tools_plan[0]['root'] == root else self.tools_snapshots.get(root, [])
                    result = scan(self.store, root, cancelled=cancel, progress=progress, force=force)
                    if result['status'] == 'complete' and snapshot and not cancel():
                        result['inspection'] = inspect_snapshot(root, snapshot, cancel, progress, self.store.preferences('organisation'))
                    last_result = result
                return last_result or dict(status='complete')
            def done(result):
                self.artist_filter.setCurrentText('All artists')
                if self._link_enabled: self._link_restart = True
                if 'inspection' in result and not self.worker.isInterruptionRequested():
                    for root in target_roots:
                        if 'inspection' in result:
                            self.tools_snapshots[root] = result['inspection']
                    curr_root = self.tools_root.currentData()
                    if curr_root in target_roots:
                        self.tools_plan = result['inspection']; self.invalidate_tools_plan()
                        self.tools_inspection_status.setText(f'Inspection reconciled with the updated library · {datetime.now():%H:%M}')
                for root in target_roots:self._library_content_changed(root,refresh=False)
                self.refresh()
            roots_label = ', '.join(Path(r).name or r for r in target_roots)
            self.job(work, done, label=f'Scan library · {roots_label}', local=True)
        else:
            QMessageBox.information(self, 'Choose a library', 'Choose the active library in Settings to update.')

    def import_catalogue(self):
        if self.demo_mode: return self.demo_notice()
        path, _ = QFileDialog.getOpenFileName(self, 'Import existing organizer catalogue', str(Path.home() / '.config/music-organizer'), 'SQLite (*.sqlite3 *.db);;All files (*)')
        if path: self.job(lambda cancel, progress: f'Imported {import_legacy(self.store, path)} saved files', label='Import organizer catalogue')

    def demo_notice(self):
        QMessageBox.information(self, 'Fictional demo', 'Launch without --demo to use your real library and Online account.')

    def find_candidates(self):
        selected = sorted({index.row() for index in self.artist_table.selectionModel().selectedRows()
                           if not self.artist_table.isRowHidden(index.row())})
        if not selected:
            QMessageBox.information(self, 'Select artists', 'Select one or more artist rows, or choose Match all artists.'); return
        self.match_artists([self.artist_rows[source_row(self.artist_table,row)] for row in selected])

    def match_all(self):
        self.match_artists(list(self.artist_rows), resume=True)

    def match_artists(self, artists, resume=False):
        if not artists: return
        preferences = self.store.match_preferences()
        def work(cancel, progress):
            if self.demo_mode:
                class DemoApi:
                    def artist(self, ident, progress): return demo.catalogue()
                    def search(self, name): return []
                api = DemoApi()
                favourites=lambda cancel,progress:[dict(id='900001', name='North Assembly')]
            else:
                api=self.api(cancel,progress); favourites=self.load_favourites
            matcher=BatchMatcher(self.store,api,self.market,favourites,preferences,self.credentials.redact)
            return matcher.run(artists,cancel,progress,resume,updated=self.review_updated.emit)
        def finished(message):
            self.batch_status.setText(message)
            self.log('Review saved candidates by selecting an artist and choosing Review candidates. No acquisition items were approved.')
            if len(artists) == 1:
                name = artists[0][0]
                rows = self.store.rows('SELECT * FROM match_reviews WHERE artist=?', (name,))
                if rows:
                    try:
                        res = json.loads(rows[0]['payload'])
                        if not res.get('candidates'):
                            ans = QMessageBox.question(
                                self, 'No Direct Matches Found',
                                f'No direct catalogue matches were found for “{name}”.\n\n'
                                'Would you like to run Extended Review to search by recording title and ISRC across releases?',
                                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No
                            )
                            if ans == QMessageBox.StandardButton.Yes:
                                self.extended_artist_review(target_artist=name)
                    except Exception:
                        pass
        self.job(work,finished,label=f'Match {len(artists)} artists')

    def review_candidates(self):
        selected=self.selected_artist()
        if not selected:return
        name,tracks=selected
        rows=self.store.rows('SELECT * FROM match_reviews WHERE artist=?',(name,))
        if not rows:
            mapping=self.store.rows('SELECT * FROM mappings WHERE artist=?',(name,))
            cached=self.store.cache(mapping[0]['tidal_id'],self.market) if mapping and mapping[0]['tidal_id'] else None
            if cached:
                self.candidates_dialog(name,resolve(tracks,[cached])[0]);return
            QMessageBox.information(self,'No saved candidates','Run Match selected first.');return
        result=json.loads(rows[0]['payload'])
        if result['market'] != self.market:
            QMessageBox.information(self,'Different market','Match this artist again for the current market.');return
        if not result['candidates']:
            msg = rows[0]['error'] or f'No direct candidates found for “{name}”. Use Link by Online ID or retry matching.'
            ans = QMessageBox.question(
                self, 'No Direct Matches',
                f'{msg}\n\nWould you like to run an Extended Review to cross-reference tracks by recording ISRC and title?',
                QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No
            )
            if ans == QMessageBox.StandardButton.Yes:
                self.extended_artist_review(target_artist=name)
            return
        self.candidates_dialog(name,result['candidates'])

    def candidates_dialog(self, name, scored):
        dialog = QDialog(self); dialog.setWindowTitle(f'Matching Candidates for “{name}” (Local Artist)'); dialog.resize(780, 400)
        layout = QVBoxLayout(dialog)
        layout.addWidget(QLabel(f'Local Library Artist: <b>{name}</b><br>Select matching online identities below. Confirming links this artist:'))
        candidates = table(['Online Artist', 'Online ID', 'Match Score', 'Matching Evidence']); layout.addWidget(candidates)
        candidates.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        fill(candidates, [(s['artist']['name'], s['artist']['id'], s['score'], s['evidence']) for s in scored])
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        buttons.button(QDialogButtonBox.StandardButton.Ok).setText('Confirm selected identities')
        buttons.accepted.connect(dialog.accept); buttons.rejected.connect(dialog.reject); layout.addWidget(buttons)
        if dialog.exec() and candidates.currentRow() >= 0:
            chosen = [scored[source_row(candidates,index.row())] for index in candidates.selectionModel().selectedRows()]
            if not chosen: return
            for result in chosen:
                if not result['artist'].get('unverified_summary'):
                    self.store.save_catalogue(result['artist']['id'], self.market, result['artist'])
            evidence = '; '.join(f"{r['artist']['id']}: {r['evidence']}" for r in chosen)
            self.store.mapping(name, chosen[0]['artist']['id'], 'confirmed', evidence, True,
                               extra_ids=[r['artist']['id'] for r in chosen[1:]])
            self.refresh()
            self.log(f'Artist identities confirmed · {name} · {len(chosen)} IDs linked')

    def extended_artist_review(self, target_artist=None):
        if isinstance(target_artist, bool):
            target_artist = None
        if self.demo_mode: return self.demo_notice()
        selected = self.selected_artist() if target_artist is None else next(((a, t) for a, t in getattr(self, 'artist_rows', []) if a == target_artist), None)
        if not selected and target_artist:
            files = []
            for r in self.store.rows("SELECT path, metadata FROM local_files WHERE present=1"):
                try:
                    meta = json.loads(r['metadata'])
                    if (meta.get('albumartist') or meta.get('artist') or '').casefold() == target_artist.casefold():
                        files.append(dict(meta, path=r['path']))
                except Exception:
                    continue
            if files:
                selected = (target_artist, files)
        if not selected:
            QMessageBox.information(self, 'Select an artist', 'Select an artist from the table to perform an extended review.')
            return
        name, local_tracks = selected
        if not local_tracks:
            QMessageBox.information(self, 'No tracks', f'No local tracks found for artist “{name}”.')
            return

        def work(cancel, progress):
            from .extended_review import discover
            return discover(local_tracks,self.store,self.market,self.api(cancel,progress),cancel,progress)

        def done(discoveries):
            if self.worker and self.worker.isInterruptionRequested():return
            dlg = ExtendedArtistReviewDialog(self, name, local_tracks, discoveries)
            if dlg.exec() and dlg.chosen_discovery:
                self.retag_and_move_tracks(name, dlg.chosen_discovery)

        self.job(work, done, label=f'Extended review · {name}')

    def retag_and_move_tracks(self, original_artist, discovery):
        if self.demo_mode:return self.demo_notice()
        from .extended_review import repair_preview,apply_repairs
        def ready(plans):
            if self.worker and self.worker.isInterruptionRequested():return
            def review():
                dialog=QDialog(self);dialog.setWindowTitle('Review tag changes and file moves');dialog.resize(850,560)
                layout=QVBoxLayout(dialog)
                layout.addWidget(QLabel(f'{len(plans)} verified file(s). Only the changes listed below will be applied.'))
                details=QPlainTextEdit();details.setReadOnly(True)
                sections=[]
                for row in plans:
                    changes='\n'.join(f'{key}: {", ".join(row["tags"].get(key,[])) or "(missing)"} → {", ".join(values)}' for key,values in row['changes'].items())
                    sections.append(f'{row["path"]}\n{changes}\nMove to: {row["target"]}')
                details.setPlainText('\n\n'.join(sections));layout.addWidget(details)
                buttons=QDialogButtonBox(QDialogButtonBox.StandardButton.Ok|QDialogButtonBox.StandardButton.Cancel)
                buttons.button(QDialogButtonBox.StandardButton.Ok).setText('Apply these changes')
                buttons.accepted.connect(dialog.accept);buttons.rejected.connect(dialog.reject);layout.addWidget(buttons)
                if not dialog.exec():return
                def work(cancel,progress):return apply_repairs(self.store,self.market,plans,cancel,progress)
                def finished(count):
                    # Discard previews of old paths; the next operation refreshes only changed files.
                    roots={row['root'] for row in plans}
                    for root in roots:self.tools_snapshots.pop(root,None);self._dirty_roots.add(root)
                    self.tools_plan=[];self.invalidate_tools_plan();self.refresh()
                    if original_artist not in self.store.artists(include_compilations=True):
                        with self.store.connect() as db:
                            for table_name in ('match_reviews','mappings','additional_mappings'):
                                db.execute(f'DELETE FROM {table_name} WHERE artist=?',(original_artist,))
                    self.log(f'Repaired {count}/{len(plans)} files · tags, paths and database links updated')
                self.job(work,finished,label='Apply reviewed repair',local=True,is_disk_op=True)
            if self.worker:self._after_job=review
            else:QTimer.singleShot(0,review)
        def start():self.job(lambda cancel,progress:repair_preview(self.store,discovery,cancel),ready,label='Prepare repair preview',local=True)
        if self.worker:self._after_job=start
        else:start()

    def link_id(self):
        import re
        from PySide6.QtWidgets import QInputDialog
        selected = self.selected_artist()
        if not selected: return
        raw, ok = QInputDialog.getText(self, 'Link Artist ID or URL', 'Artist or Album ID or web URL:')
        if not ok or not raw.strip(): return
        match = re.search(r'(\d+)', raw.strip())
        if not match:
            QMessageBox.warning(self, 'Invalid ID or URL', 'Please enter a numeric ID or a web URL containing digits (e.g. 12345 or https://tidal.com/album/171327615).')
            return
        ident = match.group(1)
        if self.demo_mode: return self.demo_notice()
        name, tracks = selected
        raw_str = raw.strip().lower()
        is_album_hint = '/album/' in raw_str

        def work(cancel, progress):
            api = self.api(cancel, progress)
            if is_album_hint:
                rel = api.album_tag_details({'id': ident})
                return ('album', rel)
            try:
                artist = api.artist(ident, progress)
                return ('artist', artist)
            except Exception:
                try:
                    rel = api.album_tag_details({'id': ident})
                    return ('album', rel)
                except Exception:
                    raise

        def done(result):
            kind, payload = result
            if kind == 'artist':
                self.candidates_dialog(name, resolve(tracks, [payload])[0])
            elif kind == 'album':
                from .extended_review import discovery_for
                discovery=discovery_for(payload,tracks)
                if discovery:self.retag_and_move_tracks(name,discovery)
                else:self.log('No verified recording on this release matches the selected local files; nothing changed.')

        self.job(work, done, label=f'Look up Online ID · {ident}')

    def unlink(self):
        selected = self.selected_artist()
        if selected:
            if not self.store.rows('SELECT artist FROM match_reviews WHERE artist=?',(selected[0],)):
                previous=[self.store.cache(m['tidal_id'],self.market) for m in self.store.linked_mappings() if m['artist']==selected[0]]
                if any(previous):self.store.save_match_review(selected[0],'review',resolve(selected[1],[p for p in previous if p])[0],self.market)
            self.store.mapping(selected[0], None, 'unlinked', 'Manually unlinked; automatic matching disabled until confirmation', True)
            self.refresh()

    def sync_artist(self):
        if self.demo_mode: return self.demo_notice()
        selected = self.selected_artist()
        if not selected: return
        mapping = [m for m in self.store.linked_mappings() if m['artist'] == selected[0]]
        if not mapping:
            self.log('Choose a linked artist before refreshing its releases.');return
        market=self.market
        def work(cancel, progress):
            api = self.api(cancel, progress)
            for link in mapping:
                ident = link['tidal_id']
                artist = api.artist(ident, progress)
                if cancel():return 'Refresh cancelled; existing catalogue retained'
                self.store.save_catalogue(ident, market, artist)
            return 'Release list refreshed · track details will load only when linking needs them'
        self.job(work, lambda _:self.refresh(), label=f'Refresh release list · {selected[0]}')

    def refresh_coverage(self):
        if not hasattr(self,'coverage_table') or self._view_data is None:return
        decisions={r['id']:r['decision'] for r in self.store.rows('SELECT id,decision FROM queue')}
        c_filter = self.copyright_filter.currentText() if hasattr(self, 'copyright_filter') else 'All copyrights'
        recommendation_filter=self.recommendation_filter.currentText() if hasattr(self,'recommendation_filter') else 'All recommendations'
        filters=(self.timeline.currentText(),self.search.text(),self.filter.currentText(),self.kind.currentText(),c_filter,recommendation_filter)
        self._coverage_pending=(dict(self._view_data,link_cache_days=self.link_cache_age.currentData()),decisions,filters)
        if not self._coverage_worker:self._start_coverage()

    def _start_coverage(self):
        data,decisions,filters=self._coverage_pending;self._coverage_pending=None
        def prepare(cancel=lambda:False,progress=lambda _:None):return data,filter_coverage(data,decisions,filters,self.demo_mode)
        if len(data['compared'])<200:self._coverage_ready(prepare());return
        self.cache_status.setText('Updating release comparison…')
        self._coverage_worker=Worker(prepare)
        self._coverage_worker.result.connect(self._coverage_ready,Qt.ConnectionType.QueuedConnection)
        self._coverage_worker.failure.connect(self.log,Qt.ConnectionType.QueuedConnection)
        self._coverage_worker.finished.connect(self._coverage_finished,Qt.ConnectionType.QueuedConnection)
        self._coverage_worker.start()

    @Slot()
    def _coverage_finished(self):
        worker=self._coverage_worker;self._coverage_worker=None
        if worker:
            worker.wait(2000)
            worker.deleteLater()
        if self._coverage_pending:self._start_coverage()

    @Slot(object)
    def _coverage_ready(self,result):
        if self._coverage_pending:return
        data,(candidates,rows,available)=result
        self.validation_candidates=candidates
        self._base_coverage_rows=rows
        if len(self.metrics)>=6:self.metrics[4].setText(f'{available:,}\nAvailable releases')
        if hasattr(self, 'card_gaps_metric'):
            self.card_gaps_metric.setText(f'{available:,} available' if available else '0 available')
            timeline_mode = self.timeline.currentText() if hasattr(self, 'timeline') else ''
            if 'Newer' in timeline_mode:
                sub_label = f'{available:,} newer releases missing locally' if available else 'Library is up to date'
            else:
                sub_label = f'{available:,} catalogue releases missing locally' if available else 'Library is up to date'
            self.card_gaps_sub.setText(sub_label)
        caches = data.get('caches', [])
        label = f'Cached catalogue · oldest snapshot {format_user_datetime(caches[0]["fetched"])}' if caches else 'No cached catalogue yet'
        self.cache_status.setText(label)
        self._render_coverage_table()
        self.refresh_overview_tables()
        self._update_missing_actions()

    def _render_coverage_table(self):
        widget = self.coverage_table
        def row_key(r):
            first = widget.item(r, 0)
            if first and first.data(Qt.ItemDataRole.UserRole) is not None: return (first.data(Qt.ItemDataRole.UserRole),)
            if first and first.text(): return (first.text(),)
            return tuple(widget.item(r, c).text() if widget.item(r, c) else '' for c in range(1, min(3, widget.columnCount())))
        selected_keys = {row_key(index.row()) for index in widget.selectionModel().selectedRows()}
        scroll=(widget.verticalScrollBar().value(),widget.horizontalScrollBar().value())

        table_rows = []
        keys = []
        row_types = {}
        row_tooltips = {}
        flattened_rows = []
        queued_ids = {str(r['id']) for r in self.store.rows("SELECT id FROM queue WHERE decision='queued'")}
        ignored_track_ids = {str(r['id']).split('track:')[1] for r in self.store.rows("SELECT id FROM queue WHERE decision='ignored' AND id LIKE 'track:%'")}

        for rel_item in self._base_coverage_rows:
            rel = rel_item['release']
            rel_id = str(rel['id'])
            if rel_id in self._missing_selection_items:self._missing_selection_items[rel_id]=rel_item
            is_expanded = rel_id in self.expanded_releases
            expand_prefix = '▼ ' if is_expanded else '▶ '

            track_count_val = rel.get('track_count') if rel.get('track_count') is not None else (len(rel['tracks']) if rel.get('tracks_loaded') else 'Unknown')
            artist_display = expand_prefix + (rel.get('artist') or 'Unknown')
            is_rel_queued = rel_id in queued_ids
            state_display = 'Queued' if is_rel_queued else rel_item.get('state', '')
            t_row = [
                artist_display,
                rel.get('title', ''),
                rel.get('date', ''),
                format_release_type(rel.get('type')),
                track_count_val,
                state_display,
                rel.get('quality', '—') or '—',
                rel_id,
                rel_item.get('recommendation',{}).get('badge','Potential'), ''
            ]
            row_idx = len(table_rows)
            table_rows.append(t_row)
            keys.append(rel_id)
            row_types[row_idx] = 'release'
            flattened_rows.append(rel_item)

            reason = rel_item.get('reason', '')
            if is_rel_queued:
                reason = ('In download queue · ' + reason) if reason else 'In download queue'
            if rel_item.get('copyright'):
                c_status = 'Matches local copyright' if rel_item.get('copyright_match') is True else ('Differs from local' if rel_item.get('copyright_match') is False else 'No local baseline')
                reason += f' · Copyright: {rel_item["copyright"]} ({c_status})'
            row_tooltips[(row_idx, 5)] = reason
            row_tooltips[(row_idx, 8)] = 'Catalogue recommendation, not a guarantee. '+ '; '.join(rel_item.get('recommendation',{}).get('evidence',[]))

            if is_expanded:
                tracks = rel.get('tracks', [])
                if not tracks and self.demo_mode:
                    cnt = int(track_count_val) if str(track_count_val).isdigit() else 4
                    tracks = [dict(id=f"{rel_id}-{t}", title=f"Track {t}", duration=180 + t*15, track_number=t) for t in range(1, cnt + 1)]
                missing_track_ids = {str(t.get('id')) for t in rel_item.get('missing', [])}
                for t_idx, track in enumerate(tracks, 1):
                    t_num = track.get('track_number') or t_idx
                    t_dur = track.get('duration') or 0
                    dur_str = f"{int(t_dur)//60}:{int(t_dur)%60:02d}" if t_dur else '—'
                    t_id = str(track.get('id', ''))
                    is_track_ignored = t_id in ignored_track_ids
                    is_missing = t_id in missing_track_ids if missing_track_ids else (rel_item.get('state') == 'Missing release')
                    if is_track_ignored:
                        status_str = 'Ignored'
                    elif is_rel_queued and is_missing:
                        status_str = 'Queued'
                    elif is_missing:
                        status_str = 'Missing'
                    else:
                        status_str = 'Present locally'
                    
                    track_table_row = [
                        f"     ↳ #{t_num}",
                        str(track.get('title', '')),
                        dur_str,
                        '',
                        '',
                        status_str,
                        rel.get('quality', '—') or '—',
                        t_id, '', ''
                    ]
                    tr_idx = len(table_rows)
                    table_rows.append(track_table_row)
                    keys.append(f"{rel_id}:{t_id}")
                    row_types[tr_idx] = 'track'
                    flattened_rows.append({'is_track': True, 'track': track, 'parent': rel_item, 'release': rel})

        self.coverage_rows = flattened_rows
        self._coverage_by_key=dict(zip(keys,flattened_rows))
        states={}
        for key,item in self._coverage_by_key.items():
            ident=str(item['release']['id']);selected=self._missing_selection.get(ident,set())
            if item.get('is_track'):
                active=selected is None or str(item['track']['id']) in selected
                states[key]=Qt.CheckState.Checked if active else Qt.CheckState.Unchecked
            else:states[key]=Qt.CheckState.Checked if selected is None else (Qt.CheckState.PartiallyChecked if selected else Qt.CheckState.Unchecked)
        self.coverage_table.model().check_states=states
        self.queue_selected_btn.setText(f"Queue selected ({sum(v is None or bool(v) for v in self._missing_selection.values())})")
        self.coverage_table.model().replace_custom(table_rows, keys, row_types, row_tooltips)
        widget.clearSelection()
        if selected_keys:
            select_rows(widget, (i for i in range(widget.rowCount()) if row_key(i) in selected_keys))

        widget.verticalScrollBar().setValue(scroll[0]);widget.horizontalScrollBar().setValue(scroll[1])

    def _missing_checked(self,key,checked):
        item=self._coverage_by_key.get(key)
        if not item:return
        ident=str(item['release']['id'])
        self._missing_selection_items[ident]=item.get('parent',item)
        if not item.get('is_track'):self._missing_selection[ident]=None if checked else set()
        else:
            selected=self._missing_selection.get(ident,set())
            if selected is None:selected={str(t['id']) for t in item['release'].get('tracks',[])}
            else:selected=set(selected)
            track=str(item['track']['id'])
            if checked:selected.add(track)
            else:selected.discard(track)
            all_ids={str(t['id']) for t in item['release'].get('tracks',[])}
            self._missing_selection[ident]=None if all_ids and selected==all_ids else selected
        self._render_coverage_table()

    def _select_missing_visible(self,checked):
        if not checked:
            self._missing_selection.clear();self._missing_selection_items.clear();self._render_coverage_table();return
        for item in self._base_coverage_rows:
            ident=str(item['release']['id']);self._missing_selection[ident]=None if checked else set()
            self._missing_selection_items[ident]=item
        self._render_coverage_table()

    def queue_checked_missing(self):
        # Draft checkboxes never start a download. Availability must already be
        # verified; queue approval remains an explicit, separate operation.
        queued=0;skipped=0
        for item in list(self._missing_selection_items.values()):
            release=item['release'];ident=str(release['id']);selected=self._missing_selection.get(ident,set())
            if selected is not None and not selected:continue
            if release.get('available') is not True:skipped+=1;continue
            tracks=None if selected is None else [t for t in release.get('tracks',[]) if str(t['id']) in selected]
            if tracks==[]:skipped+=1;continue
            self.store.enqueue(release,tracks);self._missing_selection.pop(ident,None);self._missing_selection_items.pop(ident,None);queued+=1
        self.refresh_queue();self.refresh_coverage()
        self.log(f'Queued {queued} releases for approval · {skipped} need availability or track details checked')

    def expand_release_id(self,ident):
        model=self.coverage_table.model()
        if ident in model.keys:self.toggle_release_expansion(self.coverage_table.item(model.keys.index(ident),0))

    def toggle_release_expansion(self, cell=None):
        if cell is not None and cell.column()==self.coverage_table.model().check_column:return
        row = cell.row() if hasattr(cell, 'row') else self.coverage_table.currentRow()
        if not (0 <= row < len(self.coverage_rows)): return
        item = self._coverage_item(row)
        if item.get('is_track'):
            self.review_release(item.get('parent'))
            return
        rel = item['release']
        rel_id = str(rel['id'])
        if rel_id in self.expanded_releases:
            self.expanded_releases.remove(rel_id)
            self._render_coverage_table()
        else:
            if not rel.get('tracks_loaded') and not rel.get('tracks') and not self.demo_mode:
                if self.worker:
                    self._pending_disclosure=rel_id
                    self.log('Track details will load after the current operation finishes.');return
                def work(cancel, progress):
                    detailed = self.api(cancel, progress).release_details(rel)
                    self.save_release_detail(item, detailed)
                    return detailed
                def done(detailed):
                    rel.update(detailed)
                    item['release'] = rel
                    for current in self._base_coverage_rows:
                        if str(current['release']['id'])==rel_id:current['release'].update(detailed)
                    for current in (self._view_data or {}).get('compared',[]):
                        if str(current['release']['id'])==rel_id:current['release'].update(detailed)
                    self.expanded_releases.add(rel_id)
                    self._render_coverage_table()
                self.job(work, done, label=f'Load tracks · {rel["title"]}')
            else:
                self.expanded_releases.add(rel_id)
                self._render_coverage_table()

    def _update_missing_actions(self):
        items = self.selected_coverage_items()
        filter_text = self.filter.currentText() if hasattr(self, 'filter') else ''
        if not hasattr(self, 'restore_btn') or not hasattr(self, 'ignore_btn'):
            return
        if not items:
            self.restore_btn.setVisible(filter_text == 'Ignored')
            self.ignore_btn.setVisible(filter_text != 'Ignored')
            self.ignore_btn.setText('Ignore release')
            return
        track_items = [it for it in items if it.get('is_track')]
        if track_items:
            ignored_ids = {r['id'] for r in self.store.rows("SELECT id FROM queue WHERE decision='ignored' AND id LIKE 'track:%'")}
            all_ignored = all(f"track:{it['track'].get('id')}" in ignored_ids for it in track_items)
            noun = f"{len(track_items)} tracks" if len(track_items) > 1 else "track"
            self.restore_btn.setVisible(all_ignored)
            self.ignore_btn.setVisible(not all_ignored)
            self.restore_btn.setText(f'Restore {noun}')
            self.ignore_btn.setText(f'Ignore {noun}')
        else:
            all_ignored = (filter_text == 'Ignored') or all(it.get('state') == 'Ignored' for it in items)
            noun = f"{len(items)} releases" if len(items) > 1 else "release"
            self.restore_btn.setVisible(all_ignored)
            self.ignore_btn.setVisible(not all_ignored)
            self.restore_btn.setText(f'Restore {noun}')
            self.ignore_btn.setText(f'Ignore {noun}')

    def sync_all_catalogues(self):
        if self.demo_mode: return self.demo_notice()
        mappings = self.store.linked_mappings()
        if not mappings:
            QMessageBox.information(self, 'No linked artists', 'Link or confirm artists in Link Artists before scanning for new releases.')
            return
        unique_ids = sorted({m['tidal_id'] for m in mappings if m.get('tidal_id')})
        if not unique_ids:
            QMessageBox.information(self, 'No linked artists', 'No confirmed Online artist IDs found to scan.')
            return
        def work(cancel, progress):
            api = self.api(cancel, progress)
            updated=0;failures=[]
            for i, ident in enumerate(unique_ids, 1):
                if cancel(): break
                progress(f'Syncing catalogue {i}/{len(unique_ids)} · Online ID {ident}')
                try:
                    artist = api.artist(ident, progress)
                    self.store.save_catalogue(ident, self.market, artist)
                    updated+=1
                except CatalogueError as exc:
                    if exc.batch_fatal or exc.status in (401,403,429):raise
                    failures.append(str(ident));progress(f'Could not update artist {ident} · {exc}')
                except Exception as exc:
                    failures.append(str(ident));progress(f'Could not update artist {ident} · {exc}')
            suffix=f' · {len(failures)} need retry' if failures else ''
            return f'Catalogue scan complete · {updated} artists updated{suffix}.'
        def done(msg):
            self.refresh()
            self.log(msg)
        self.job(work, done, label=f'Scan for new releases · {len(unique_ids)} artists')

    def check_release_links(self, checked=False, force_selected=False):
        if self.demo_mode or self.worker: return
        if self._view_worker or self._coverage_worker:
            self.log('Release comparison is still updating. Run the link check once the current results appear.');return
        max_age_days=self.link_cache_age.currentData()
        if force_selected:
            selected=self.selected_release()
            if not selected:
                self.log('Select a release to recheck its link.');return
            # Invalidate only the job's copy; a failed request must preserve the saved check.
            items=[dict(selected,release=dict(selected['release'],link_checked_at=0))]
        else:items=list(getattr(self,'validation_candidates',[]))
        if not items or not needs_verification(items,max_age_days):
            self.log('The filtered release links are already checked; cached results reused.');return
        self.job(lambda cancel,progress:verify_releases(items,self.api(cancel,progress),self.save_release_detail,cancel,progress,
                                                        max_requests=1 if force_selected else 100,max_age_days=max_age_days),
                 label='Recheck selected release link' if force_selected else 'Check up to 100 release links · newest first')

    def _coverage_item(self,row):
        model=self.coverage_table.model()
        if 0 <= row < len(model.keys):return getattr(self,'_coverage_by_key',{}).get(model.keys[row],{})
        return {}

    def selected_coverage_items(self):
        sel_model = self.coverage_table.selectionModel()
        indexes = sel_model.selectedRows() if sel_model else []
        items = []
        for idx in indexes:
            row = idx.row()
            if 0 <= row < len(self.coverage_rows):
                items.append(self._coverage_item(row))
        if not items:
            row = self.coverage_table.currentRow()
            if 0 <= row < len(self.coverage_rows):
                items.append(self._coverage_item(row))
        return items

    def selected_release(self):
        row = self.coverage_table.currentRow()
        if 0 <= row < len(self.coverage_rows):
            item = self._coverage_item(row)
            if item.get('is_track'):
                return item.get('parent')
            return item
        return None

    def selected_releases(self):
        sel_model = self.coverage_table.selectionModel()
        indexes = sel_model.selectedRows() if sel_model else []
        items = []
        seen_ids = set()
        for idx in indexes:
            row = idx.row()
            if 0 <= row < len(self.coverage_rows):
                item = self._coverage_item(row)
                if item.get('is_track'):
                    item = item.get('parent')
                if item and item.get('release'):
                    rel_id = str(item['release']['id'])
                    if rel_id not in seen_ids:
                        seen_ids.add(rel_id)
                        items.append(item)
        if not items:
            single = self.selected_release()
            if single and single.get('release'):
                items = [single]
        return items

    def save_release_detail(self, item, release):
        cached=self.store.preferences(f'tag-review:{self.market}:{release["id"]}')
        detail=dict(cached,**release)
        if release.get('tracks_loaded'):
            detail['details_checked_at']=time.time()
            self.store.save_preferences(f'tag-review:{self.market}:{release["id"]}',detail)
        for ident in item.get('artist_ids') or ([item['artist_id']] if item.get('artist_id') else []):
            artist = self.store.cache(ident, self.market)
            if artist:
                artist['releases'] = [release if str(r['id']) == str(release['id']) else r for r in artist['releases']]
                with self.store.connect() as db:
                    db.execute('UPDATE catalogue SET payload=? WHERE artist_id=? AND market=?',
                               (json.dumps(artist), ident, self.market))

    def check_release_tracks(self, checked=False, review=False):
        item = self.selected_release()
        if not item: return
        def work(cancel, progress):
            release = self.api(cancel, progress).release_details(item['release'])
            self.save_release_detail(item, release)
            artist = dict(releases=[release])
            local = self.store.artists()
            tracks = [track for name in item['local_artists'] for track in local.get(name, [])]
            result = coverage(tracks, artist)[0]
            result.update(artist_id=item['artist_id'], artist_ids=item['artist_ids'], local_artists=item['local_artists'])
            return result
        def done(result):
            self.log(f'Track check complete · {result["state"]} · {result["reason"]}')
            if review:
                self.review_release(result)
            else:
                QMessageBox.information(self, 'Track check complete', result['release']['title'] + '\n' + result['state'] + '\n' + result['reason'])
        if self.demo_mode:
            QMessageBox.information(self, 'Demo track details', 'The fictional demo already includes track details.'); return
        self.job(work, done, label='Check tracks · ' + item['release']['title'])

    def queue_whole_release(self):
        item = self.selected_release()
        if not item: return
        if item['state'] not in ('Missing release', 'Alternate edition'):
            QMessageBox.information(self, 'Review this release', 'For a locally present or partial release, check or select tracks to avoid queueing music you already have.'); return
        def confirm(release):
            if release.get('available') is not True:
                QMessageBox.information(self, 'Availability unconfirmed', 'This release cannot be queued until its availability is confirmed.'); return
            count = release.get('track_count', len(release['tracks']))
            message = f'{release["artist"]} — {release["title"]}\nWhole release ({count if count is not None else "unknown number of"} tracks).\nThis queues the album URL. Individual track ownership is not checked.\nYou will approve the queue item before exporting.'
            if QMessageBox.question(self, 'Queue whole release', message) == QMessageBox.StandardButton.Yes:
                self.store.enqueue(release); self.refresh(); self.log('Queued whole release · ' + release['title'])
        if item['release'].get('available') is None:
            def work(cancel, progress):
                release = self.api(cancel, progress).release_availability(item['release'])
                self.save_release_detail(item, release)
                return release
            self.job(work, confirm, label='Check selected release availability')
        else:
            confirm(item['release'])

    def review_release(self, checked_item=None):
        item = checked_item or self.selected_release()
        if not item: return
        if item.get('summary'):
            self.check_release_tracks(review=True); return
        self.track_selection_dialog(item['release'],item['missing'],False)

    def track_selection_dialog(self, release, selected, editing=False):
        dialog=QDialog(self);dialog.setWindowTitle('Tracks · '+release['title']);dialog.resize(950,520)
        layout=QVBoxLayout(dialog)
        info=QLabel(f"{release['artist']} · {release.get('date','')} · {release.get('type','')} · {release.get('quality','')}\nSelect the tracks to queue with Shift or Command/Ctrl. Saving a changed selection requires approval again.")
        if release.get('existing_destination'):
            dest=release['existing_destination'];info.setText(info.text()+f"\nDestination: {Path(dest['root'])/dest['album_relative']}")
        info.setWordWrap(True);layout.addWidget(info)
        tracks=release['tracks'];view=table(['#','Disc','Title','Duration','ISRC','Online ID']);view.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection);layout.addWidget(view)
        def length(t):
            seconds=int(t.get('duration') or 0);return f'{seconds//60}:{seconds%60:02d}' if seconds else '—'
        fill(view,[(t.get('track_number') or n+1,t.get('disc_number') or '—',t['title'],length(t),t.get('isrc') or '—',t['id']) for n,t in enumerate(tracks)],keys=[str(t['id']) for t in tracks])
        selected_ids={str(t['id']) for t in selected}
        for row in range(view.rowCount()):
            t=tracks[source_row(view,row)]
            if str(t['id']) in selected_ids:view.selectionModel().select(view.model().index(row,0),QItemSelectionModel.SelectionFlag.Select|QItemSelectionModel.SelectionFlag.Rows)
        buttons=QDialogButtonBox(QDialogButtonBox.StandardButton.Save|QDialogButtonBox.StandardButton.Cancel)
        buttons.button(QDialogButtonBox.StandardButton.Save).setText('Save selected tracks to queue');buttons.accepted.connect(dialog.accept);buttons.rejected.connect(dialog.reject);layout.addWidget(buttons)
        if dialog.exec():
            chosen=[tracks[source_row(view,i.row())] for i in view.selectionModel().selectedRows()]
            if not chosen:
                QMessageBox.information(self,'No tracks selected','No queue changes made. Select at least one track.');return
            if release.get('available') is not True:
                QMessageBox.information(self,'Unavailable release','Online has not confirmed this release is available.');return
            self.store.enqueue(release,chosen);self.refresh();self.log(f'Queued {len(chosen)} selected tracks · approval required')

    def edit_queue_tracks(self):
        row = self.queue_table.currentRow()
        if row < 0: return
        displayed = getattr(self, 'displayed_queue_rows', None)
        if displayed and 0 <= row < len(displayed):
            rec = displayed[row]['record']
            release = json.loads(rec['payload'])
        elif 0 <= row < len(self.queue_rows):
            release = json.loads(self.queue_rows[row]['payload'])
        else:
            return
        def done(detailed):
            selected = release.get('selected_tracks')
            self.track_selection_dialog(detailed, detailed['tracks'] if selected is None else selected, True)
        if release.get('tracks_loaded', bool(release.get('tracks'))): done(release)
        elif not self.demo_mode: self.job(lambda cancel, progress: self.api(cancel, progress).release_details(release), done, label='Load queued tracks')
        elif self.demo_mode:
            cnt = int(release.get('track_count', 4)) if str(release.get('track_count')).isdigit() else 4
            release['tracks'] = [dict(id=f"{release['id']}-{t}", title=f"Track {t}", duration=180 + t*15, track_number=t) for t in range(1, cnt + 1)]
            done(release)

    def toggle_queue_expansion(self, item=None):
        if self.worker:return
        if hasattr(item,'column') and item.column()==0:return
        row = item.row() if hasattr(item, 'row') else self.queue_table.currentRow()
        displayed = getattr(self, 'displayed_queue_rows', None)
        if not displayed or not (0 <= row < len(displayed)): return
        row_data = displayed[row]
        if row_data['type'] != 'release':
            track_id = row_data['track'].get('id')
            parent_id = row_data['parent_id']
            payload = json.loads(row_data['record']['payload'])
            sel = payload.get('selected_tracks')
            if sel is None:
                self.toggle_queue_track_selection(parent_id, track_id, False)
            else:
                sel_ids = {str(t.get('id')) for t in sel}
                self.toggle_queue_track_selection(parent_id, track_id, str(track_id) not in sel_ids)
            return

        rel_id = str(row_data['id'])
        if rel_id in self.expanded_queue_releases:
            self.expanded_queue_releases.remove(rel_id)
            self.refresh_queue(refresh_overview=False)
        else:
            payload = json.loads(row_data['record']['payload'])
            if not payload.get('tracks_loaded') and not payload.get('tracks') and not self.demo_mode:
                def work(cancel, progress):
                    detailed = self.api(cancel, progress).release_details(payload)
                    return detailed
                def done(detailed):
                    payload.update(detailed)
                    with self.store.connect() as db:
                        db.execute("UPDATE queue SET payload=?, updated=? WHERE id=?", (json.dumps(payload), now(), rel_id))
                    self.expanded_queue_releases.add(rel_id)
                    self.refresh_queue(refresh_overview=False)
                self.job(work, done, label=f'Load tracks · {payload.get("title", "")}')
            else:
                if not payload.get('tracks') and self.demo_mode:
                    cnt = int(payload.get('track_count', 4)) if str(payload.get('track_count')).isdigit() else 4
                    payload['tracks'] = [dict(id=f"{rel_id}-{t}", title=f"Track {t}", duration=180 + t*15, track_number=t) for t in range(1, cnt + 1)]
                    with self.store.connect() as db:
                        db.execute("UPDATE queue SET payload=?, updated=? WHERE id=?", (json.dumps(payload), now(), rel_id))
                self.expanded_queue_releases.add(rel_id)
                self.refresh_queue(refresh_overview=False)

    def toggle_queue_track_selection(self, rel_id, track_id, checked):
        rows = self.store.rows("SELECT * FROM queue WHERE id=? AND decision='queued'", (str(rel_id),))
        if not rows: return
        r = rows[0]
        payload = json.loads(r['payload'])
        all_tracks = payload.get('tracks', [])
        if not all_tracks and self.demo_mode:
            cnt = int(payload.get('track_count', 4)) if str(payload.get('track_count')).isdigit() else 4
            all_tracks = [dict(id=f"{rel_id}-{t}", title=f"Track {t}", duration=180 + t*15, track_number=t) for t in range(1, cnt + 1)]
            payload['tracks'] = all_tracks

        current_sel = payload.get('selected_tracks')
        if not r['approved']:current_sel=[]
        if current_sel is None:
            selected_map = {str(t.get('id')): t for t in all_tracks}
        else:
            selected_map = {str(t.get('id')): t for t in current_sel}

        t_id_str = str(track_id)
        if checked:
            for t in all_tracks:
                if str(t.get('id')) == t_id_str:
                    selected_map[t_id_str] = t
                    break
        else:
            selected_map.pop(t_id_str, None)

        if len(selected_map) == len(all_tracks) and len(all_tracks) > 0:
            payload['selected_tracks'] = None
            approved = 1
        elif len(selected_map) == 0:
            payload['selected_tracks'] = []
            approved = 0
        else:
            payload['selected_tracks'] = list(selected_map.values())
            approved = 1

        with self.store.connect() as db:
            db.execute("UPDATE queue SET payload=?, approved=?, updated=? WHERE id=?",
                       (json.dumps(payload), approved, now(), str(rel_id)))
        self.refresh_queue()

    def ignore_release(self):
        items = self.selected_coverage_items()
        if not items: return
        track_items = [it for it in items if it.get('is_track')]
        with self.store.connect() as db:
            if track_items:
                for it in track_items:
                    t = it['track']
                    db.execute("INSERT INTO queue VALUES(?,?,0,'ignored',?) ON CONFLICT(id) DO UPDATE SET decision='ignored',approved=0,updated=excluded.updated",
                               (f"track:{t['id']}", json.dumps(t), now()))
                self.log(f'Ignored {len(track_items)} track(s).')
            else:
                rel_items = self.selected_releases()
                for item in rel_items:
                    release = item['release']
                    db.execute("INSERT INTO queue VALUES(?,?,0,'ignored',?) ON CONFLICT(id) DO UPDATE SET decision='ignored',approved=0,updated=excluded.updated",
                               (release['id'], json.dumps(release), now()))
                self.log(f'Ignored {len(rel_items)} release(s).')
        self.refresh()

    def restore_release(self):
        items = self.selected_coverage_items()
        if not items: return
        track_items = [it for it in items if it.get('is_track')]
        with self.store.connect() as db:
            if track_items:
                for it in track_items:
                    t = it['track']
                    db.execute("DELETE FROM queue WHERE id=? AND decision='ignored'", (f"track:{t['id']}",))
                self.log(f'Restored {len(track_items)} track(s).')
            else:
                rel_items = self.selected_releases()
                for item in rel_items:
                    db.execute("DELETE FROM queue WHERE id=? AND decision='ignored'", (item['release']['id'],))
                self.log(f'Restored {len(rel_items)} release(s).')
        self.refresh()

    def open_release(self):
        item = self.selected_release()
        if item and not self.demo_mode: QDesktopServices.openUrl(QUrl(f"https://tidal.com/browse/album/{item['release']['id']}"))

    def _sort_queue(self,column):
        previous=getattr(self,'_queue_sort',None)
        order=Qt.SortOrder.DescendingOrder if previous==(column,Qt.SortOrder.AscendingOrder) else Qt.SortOrder.AscendingOrder
        self._queue_sort=(column,order)
        self.queue_table.horizontalHeader().setSortIndicatorShown(True)
        self.queue_table.horizontalHeader().setSortIndicator(column,order)
        self.refresh_queue()

    def refresh_queue(self,refresh_overview=True):
        scroll=(self.queue_table.verticalScrollBar().value(),self.queue_table.horizontalScrollBar().value())
        self.queue_table.blockSignals(True)
        self.queue_rows = self.store.rows("SELECT * FROM queue WHERE decision='queued' ORDER BY updated DESC")
        table_rows = []
        keys = []
        displayed_rows = []

        for r in self.queue_rows:
            payload = json.loads(r['payload'])
            rel_id = str(r['id'])
            is_expanded = rel_id in getattr(self, 'expanded_queue_releases', set())
            expand_prefix = '▼ ' if is_expanded else '▶ '

            sel = payload.get('selected_tracks')
            if sel is None:
                sel_text = "Whole release"
            elif len(sel) == 0:
                sel_text = "0 selected tracks"
            else:
                sel_text = f"{len(sel)} selected tracks"
            tracks_val = str(payload.get('track_count', len(payload.get('tracks', [])) or '—'))

            if payload.get('available') is False:
                avail_text = 'Unavailable'
            elif bool(r['approved']):
                avail_text = 'Ready to download'
            else:
                avail_text = 'Available'

            t_row = [
                '',
                expand_prefix + payload.get('artist', ''),
                payload.get('title', ''),
                sel_text,
                payload.get('date', ''),
                format_release_type(payload.get('type')),
                tracks_val,
                avail_text
            ]
            table_rows.append(t_row)
            keys.append(rel_id)
            displayed_rows.append({'type': 'release', 'record': r, 'payload': payload, 'id': rel_id})

            if is_expanded:
                tracks = payload.get('tracks', [])
                if not tracks and self.demo_mode:
                    cnt = int(tracks_val) if str(tracks_val).isdigit() else 4
                    tracks = [dict(id=f"{rel_id}-{t}", title=f"Track {t}", duration=180 + t*15, track_number=t) for t in range(1, cnt + 1)]
                
                sel_track_ids = {str(t.get('id')) for t in sel} if sel is not None else {str(t.get('id')) for t in tracks}
                for t_idx, track in enumerate(tracks, 1):
                    t_num = track.get('track_number') or t_idx
                    t_dur = track.get('duration') or 0
                    dur_str = f"{int(t_dur)//60}:{int(t_dur)%60:02d}" if t_dur else '—'
                    t_id = str(track.get('id', ''))
                    is_track_checked = bool(r['approved']) and (t_id in sel_track_ids)

                    tr_row = [
                        '',
                        f"     ↳ #{t_num}",
                        str(track.get('title', '')),
                        dur_str,
                        str(t_num),
                        format_release_type(payload.get('type')) or '',
                        '',
                        t_id
                    ]
                    table_rows.append(tr_row)
                    keys.append(f"{rel_id}:{t_id}")
                    displayed_rows.append({
                        'type': 'track',
                        'record': r,
                        'parent_id': rel_id,
                        'track': track,
                        'checked': is_track_checked
                    })

        if getattr(self,'_queue_sort',None):
            column,order=self._queue_sort;blocks=[]
            for n,row in enumerate(displayed_rows):
                if row['type']=='track' and blocks:blocks[-1].append(n)
                else:blocks.append([n])
            def sort_key(block):
                n=block[0]
                if column==0:return (0,int(displayed_rows[n]['record']['approved']))
                value=str(table_rows[n][column]).lstrip('▶▼ ').casefold()
                try:return (0,float(value.replace(',','')))
                except ValueError:return (1,value)
            blocks.sort(key=sort_key,reverse=order==Qt.SortOrder.DescendingOrder)
            indices=[n for block in blocks for n in block]
            table_rows=[table_rows[n] for n in indices];keys=[keys[n] for n in indices];displayed_rows=[displayed_rows[n] for n in indices]
        self.displayed_queue_rows = displayed_rows
        fill(self.queue_table, table_rows, keys=keys)

        for i, row_item in enumerate(displayed_rows):
            item = self.queue_table.item(i, 0)
            if item:
                item.setText('')
                item.setData(Qt.ItemDataRole.DisplayRole, '')
                item.setFlags(item.flags() | Qt.ItemFlag.ItemIsUserCheckable)
                active=bool(row_item['record']['approved']) if row_item['type']=='release' else bool(row_item['checked'])
                partial=row_item['type']=='release' and active and row_item['payload'].get('selected_tracks') is not None
                item.setCheckState(Qt.CheckState.PartiallyChecked if partial else Qt.CheckState.Checked if active else Qt.CheckState.Unchecked)
            if row_item['type']=='track':
                is_chk=bool(row_item['checked'])
                for c in range(self.queue_table.columnCount()):
                    cell = self.queue_table.item(i, c)
                    if cell:
                        font = cell.font()
                        font.setPointSize(max(9, font.pointSize() - 1))
                        cell.setFont(font)
                        cell.setBackground(QColor(255, 255, 255, 8))
                        if not is_chk:
                            cell.setForeground(QColor(142, 142, 147))
                        else:
                            cell.setForeground(QColor(140, 140, 145))

            active=bool(row_item['record']['approved']) if row_item['type']=='release' else bool(row_item['checked'])
            if not active:
                for col in range(self.queue_table.columnCount()):
                    cell=self.queue_table.item(i,col)
                    if cell:cell.setForeground(QColor(115,115,120));cell.setBackground(QColor(0,0,0,20))
        self.queue_table.blockSignals(False)
        self.filter_queue_table()
        self.queue_table.verticalScrollBar().setValue(scroll[0]);self.queue_table.horizontalScrollBar().setValue(scroll[1])
        if refresh_overview:self.refresh_overview_tables()

    def set_queue_approval(self, ident, approved):
        # A release checkbox always selects or clears the entire release.
        with self.store.connect() as db:
            record=db.execute("SELECT payload FROM queue WHERE id=? AND decision='queued'",(str(ident),)).fetchone()
            if record is not None:
                payload=json.loads(record[0]);payload['selected_tracks']=None if approved else []
                db.execute("UPDATE queue SET approved=?,payload=?,updated=? WHERE id=? AND decision='queued'",
                           (int(approved),json.dumps(payload),now(),str(ident)))
        self.refresh_queue()

    def queue_changed(self, item):
        if item.column()!=0:return
        displayed=getattr(self,'displayed_queue_rows',[])
        if not 0<=item.row()<len(displayed):return
        row=displayed[item.row()]
        checked=item.checkState()==Qt.CheckState.Checked
        # Rebuild only after Qt finishes dispatching the checkbox event.
        if row['type']=='release':
            QTimer.singleShot(0,lambda ident=row['id'],value=checked:self.set_queue_approval(ident,value))
        else:
            QTimer.singleShot(0,lambda ident=row['parent_id'],track=row['track']['id'],value=checked:self.toggle_queue_track_selection(ident,track,value))

    def remove_queue(self):
        selected = sorted({i.row() for i in self.queue_table.selectedIndexes()})
        displayed = getattr(self, 'displayed_queue_rows', None)
        ids_to_delete = set()
        if displayed:
            for i in selected:
                if 0 <= i < len(displayed):
                    ids_to_delete.add(displayed[i]['id'] if displayed[i]['type'] == 'release' else displayed[i]['parent_id'])
        else:
            for i in selected:
                if 0 <= i < len(self.queue_rows):
                    ids_to_delete.add(self.queue_rows[i]['id'])
        if ids_to_delete:
            with self.store.connect() as db:
                db.executemany('DELETE FROM queue WHERE id=?', [(ident,) for ident in ids_to_delete])
            self.refresh()

    def export_queue(self):
        from PySide6.QtWidgets import QPlainTextEdit
        content = self.store.export('txt')
        if not content:
            QMessageBox.information(self, 'Nothing approved', 'Check the approval boxes for items you want to export.'); return
        if self.demo_mode:
            QMessageBox.information(self, 'Fictional demo', 'Demo IDs are fictional. Export is disabled to prevent submitting them to another app.'); return
        dialog = QDialog(self); dialog.setWindowTitle('Preview approved export'); dialog.resize(650, 440)
        layout = QVBoxLayout(dialog); preview = QPlainTextEdit(content); preview.setReadOnly(True); layout.addWidget(preview)
        fmt = QComboBox(); fmt.addItems(['txt', 'csv', 'json', 'm3u']); layout.addWidget(fmt)
        fmt.currentTextChanged.connect(lambda f: preview.setPlainText(self.store.export(f)))
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Save | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(dialog.accept); buttons.rejected.connect(dialog.reject); layout.addWidget(buttons)
        if dialog.exec():
            extension = fmt.currentText()
            path, _ = QFileDialog.getSaveFileName(self, 'Save acquisition list', f'acquisition-queue.{extension}', f'{extension.upper()} (*.{extension})')
            if path:
                if Path(path).suffix.lower() != '.' + extension:
                    QMessageBox.warning(self, 'Choose an export filename', f'The filename must end in .{extension}.'); return
                try:
                    Path(path).write_text(self.store.export(extension), encoding='utf-8')
                    self.log(f'Approved acquisition list exported · {path}')
                except OSError:
                    QMessageBox.warning(self, 'Export failed', 'Could not write the selected file. Choose a writable location.')

    def closeEvent(self, event):
        if self.worker and self.worker.isRunning() and getattr(self, '_is_disk_operation', False):
            QMessageBox.warning(
                self,
                'Operation in progress',
                'Cannot close while files or tags are being modified on disk. Please wait for the current operation to complete or cancel it safely first.'
            )
            event.ignore()
            return
        self._closing_requested=True
        active = [getattr(self, a, None) for a in ('_link_worker', 'worker', '_preview_worker', '_view_worker', '_coverage_worker', '_startup_worker','_bg_probe_worker')]
        running=[w for w in active if w and w.isRunning()]
        if running:
            if hasattr(self, 'closing_overlay'):
                self.closing_overlay.resizeToParent()
                self.closing_overlay.show()
                self.closing_overlay.raise_()
        if hasattr(self, '_bg_linking_timer'):
            self._bg_linking_timer.stop()
        if running:
            for worker in running:worker.requestInterruption()
            event.ignore();QTimer.singleShot(250,self.close);return
        self.settings.setValue('page', self.nav.currentRow())
        try:
            QApplication.instance().styleHints().colorSchemeChanged.disconnect(self.apply_theme)
        except Exception:
            pass
        event.accept()


def get_default_app_dir():
    if sys.platform == 'darwin':
        new_dir = Path.home() / 'Library/Application Support/Tibrary'
        old_dir = Path.home() / 'Library/Application Support/TidalLibraryManager'
    else:
        new_dir = Path.home() / '.local/share/tibrary'
        old_dir = Path.home() / '.local/share/tidal-library-manager'
    if not new_dir.exists() and old_dir.exists():
        try:
            import shutil
            shutil.copytree(old_dir, new_dir)
        except Exception:
            return old_dir
    return new_dir


def main():
    parser = argparse.ArgumentParser(description='Tibrary')
    parser.add_argument('--demo', action='store_true', help='Open a separate fictional offline library')
    parser.add_argument('--db', type=Path, help='Application catalogue path')
    args = parser.parse_args()
    default = get_default_app_dir()
    store = Store(args.db or default / ('demo.sqlite3' if args.demo else 'library.sqlite3'))
    from .diagnostics import install
    install(store.path.parent/'logs')
    if args.demo: demo.seed(store)
    if sys.platform == 'darwin':
        try:
            import ctypes, ctypes.util
            cf = ctypes.cdll.LoadLibrary(ctypes.util.find_library('CoreFoundation'))
            cf.CFBundleGetMainBundle.restype = ctypes.c_void_p
            cf.CFBundleGetInfoDictionary.argtypes = [ctypes.c_void_p]
            cf.CFBundleGetInfoDictionary.restype = ctypes.c_void_p
            cf.CFStringCreateWithCString.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_uint32]
            cf.CFStringCreateWithCString.restype = ctypes.c_void_p
            cf.CFDictionarySetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
            bundle = cf.CFBundleGetMainBundle()
            if bundle:
                info = cf.CFBundleGetInfoDictionary(bundle)
                if info:
                    kCFStringEncodingUTF8 = 0x08000100
                    k = cf.CFStringCreateWithCString(None, b'CFBundleName', kCFStringEncodingUTF8)
                    v = cf.CFStringCreateWithCString(None, b'Tibrary', kCFStringEncodingUTF8)
                    cf.CFDictionarySetValue(info, k, v)
        except Exception:
            pass
    app = QApplication(sys.argv[:1])
    app.setApplicationName("Tibrary")
    app.setApplicationDisplayName("Tibrary")
    if sys.platform != 'darwin': app.setStyle('Fusion')
    window = Window(store, args.demo); window.show()
    if not args.demo: QTimer.singleShot(0, window.test_connections)
    return app.exec()
