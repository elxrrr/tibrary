"""A read-only table that creates Qt cells only for the visible viewport."""
from PySide6.QtCore import QAbstractTableModel,Qt,Signal
from PySide6.QtGui import QFont, QColor
from PySide6.QtWidgets import QTableView

class RowsModel(QAbstractTableModel):
    checkedChanged=Signal(str,bool)
    def __init__(self,headers,parent):
        super().__init__(parent);self.headers=headers;self.rows=[];self.keys=[];self.tooltips={};self.row_types={};self.sort_column=-1;self.sort_order=Qt.SortOrder.AscendingOrder;self.check_column=None;self.check_states={}
    def rowCount(self,parent=None):return 0 if parent is not None and parent.isValid() else len(self.rows)
    def columnCount(self,parent=None):return 0 if parent is not None and parent.isValid() else len(self.headers)
    def headerData(self,section,orientation,role=Qt.ItemDataRole.DisplayRole):
        if orientation==Qt.Orientation.Horizontal and role==Qt.ItemDataRole.DisplayRole:return self.headers[section]
    def data(self,index,role=Qt.ItemDataRole.DisplayRole):
        if not index.isValid():return None
        r, c = index.row(), index.column()
        row_type = self.row_types.get(r, 'release')
        if role==Qt.ItemDataRole.CheckStateRole and c==self.check_column:return self.check_states.get(str(self.keys[r]),Qt.CheckState.Unchecked)
        if role==Qt.ItemDataRole.ToolTipRole:return self.tooltips.get((r,c),str(self.rows[r][c]))
        if role==Qt.ItemDataRole.DisplayRole:return str(self.rows[r][c])
        if role==Qt.ItemDataRole.UserRole and c==0:return self.keys[r] if self.keys else None
        if role==Qt.ItemDataRole.FontRole:
            if self.headers[c]=='Recommendation' and row_type!='track':
                font=QFont();font.setBold(True);return font
            if row_type == 'track':
                font = QFont()
                font.setPointSize(max(9, font.pointSize() - 2))
                return font
        inactive=self.check_column is not None and self.check_states.get(str(self.keys[r]),Qt.CheckState.Unchecked)==Qt.CheckState.Unchecked
        if role==Qt.ItemDataRole.BackgroundRole:
            if inactive:return QColor(0,0,0,20)
            if row_type == 'track':
                return QColor(255, 255, 255, 7)
        if role==Qt.ItemDataRole.ForegroundRole:
            if inactive:return QColor(115,115,120)
            if self.headers[c]=='Recommendation':
                return QColor({'Recommended':'#24884b','Potential':'#bd8520','Suspect':'#d66a35','Unmatched':'#d65353'}.get(str(self.rows[r][c]),'#888888'))
            if row_type == 'ignored':
                return QColor(135, 135, 140)
            if row_type == 'track':
                return QColor(140, 140, 145)
    def flags(self,index):
        flags=super().flags(index)
        if index.isValid() and index.column()==self.check_column:flags|=Qt.ItemFlag.ItemIsUserCheckable
        return flags
    def setData(self,index,value,role=Qt.ItemDataRole.EditRole):
        if index.isValid() and index.column()==self.check_column and role==Qt.ItemDataRole.CheckStateRole:
            checked=value in (Qt.CheckState.Checked,Qt.CheckState.Checked.value)
            self.checkedChanged.emit(str(self.keys[index.row()]),checked)
            return True
        return False
    def replace(self,rows,keys,row_types=None):
        self.replace_custom(rows,keys,row_types)
    def replace_custom(self,rows,keys,row_types=None,tooltips=None):
        self.beginResetModel();self.rows=[list(row)+['']*(len(self.headers)-len(row)) if len(row)<len(self.headers) else row for row in rows];self.keys=keys or [];self.row_types=row_types or {};self.tooltips=tooltips or {};self.endResetModel()
        self.sort(self.sort_column,self.sort_order)
    def sort(self, column, order=Qt.SortOrder.AscendingOrder):
        if not 0 <= column < len(self.headers):return
        self.sort_column=column;self.sort_order=order
        if not self.rows:return
        # Keep child tracks with their release and preserve stable identity keys.
        blocks=[]
        for i in range(len(self.rows)):
            if self.row_types.get(i) == 'track' and blocks: blocks[-1].append(i)
            else: blocks.append([i])
        def key(block):
            if column==self.check_column:return (0,self.check_states.get(str(self.keys[block[0]]),Qt.CheckState.Unchecked).value,'')
            value=str(self.rows[block[0]][column]).strip().replace(',', '')
            try: return (0, float(value), '')
            except ValueError: return (1, 0, value.casefold())
        blocks.sort(key=key, reverse=order == Qt.SortOrder.DescendingOrder)
        indices=[i for block in blocks for i in block]
        self.beginResetModel()
        self.rows=[self.rows[i] for i in indices]
        if self.keys: self.keys=[self.keys[i] for i in indices]
        self.row_types={new:self.row_types[old] for new,old in enumerate(indices) if old in self.row_types}
        positions={old:new for new,old in enumerate(indices)}
        self.tooltips={(positions[row],col):text for (row,col),text in self.tooltips.items() if row in positions}
        self.endResetModel()

class Cell:
    def __init__(self,table,row,column):self.table,self.r,self.c=table,row,column
    def row(self):return self.r
    def column(self):return self.c
    def setToolTip(self,text):self.table.model().tooltips[self.r,self.c]=text
    def text(self):return str(self.table.model().rows[self.r][self.c])
    def data(self,role):return self.table.model().data(self.table.model().index(self.r,self.c),role)

class VirtualTable(QTableView):
    itemSelectionChanged=Signal()
    itemDoubleClicked=Signal(object)
    def __init__(self,headers):
        super().__init__();self.setModel(RowsModel(headers,self))
        self.horizontalHeader().setSortIndicator(-1,Qt.SortOrder.AscendingOrder)
        self.setSortingEnabled(True)
        self.selectionModel().selectionChanged.connect(lambda *_:self.itemSelectionChanged.emit())
        self.doubleClicked.connect(lambda index:self.itemDoubleClicked.emit(Cell(self,index.row(),index.column())))
        self.verticalHeader().setDefaultSectionSize(30)
        self.horizontalHeader().setStretchLastSection(True)
    def rowCount(self):return self.model().rowCount()
    def columnCount(self):return self.model().columnCount()
    def currentRow(self):return self.currentIndex().row()
    def item(self,row,column):
        return Cell(self,row,column) if 0<=row<self.rowCount() and 0<=column<self.columnCount() else None
