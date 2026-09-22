"""Step through recording-verified album proposals without writing music files."""
import copy
from collections import OrderedDict
from pathlib import Path
from PySide6.QtWidgets import (QDialog,QVBoxLayout,QHBoxLayout,QLabel,QComboBox,
    QPlainTextEdit,QPushButton,QCheckBox,QTableWidget,QTableWidgetItem,QHeaderView,QAbstractItemView)
from .maintenance import first
from .library_workflows import workflow_plan


def option_key(option):
    return str(option['id']),option['artist'],option['album']


def review_groups(rows):
    groups=OrderedDict()
    for row in rows:
        tags=row.get('tags',{})
        key=(row['root'],first(tags,'albumartist'),first(tags,'album'),
             tuple(sorted(option_key(o) for o in row.get('catalogue_options',[]))))
        groups.setdefault(key,[]).append(row)
    return list(groups.values())


class AlbumRepairReview(QDialog):
    def __init__(self,rows,parent=None):
        super().__init__(parent)
        self.setWindowTitle('Review album fixes');self.resize(850,650)
        self.groups=review_groups(copy.deepcopy(rows));self.position=0;self.decisions={}
        self.plans=[]
        layout=QVBoxLayout(self);self.heading=QLabel();self.heading.setWordWrap(True);layout.addWidget(self.heading)
        self.description=QLabel();self.description.setWordWrap(True);layout.addWidget(self.description)
        self.choice=QComboBox();layout.addWidget(self.choice)
        self.details=QPlainTextEdit();self.details.setReadOnly(True);layout.addWidget(self.details,1)
        self.preview=QTableWidget(0,2);self.preview.setHorizontalHeaderLabels(['File','Tag changes'])
        self.preview.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.preview.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.Stretch)
        layout.addWidget(self.preview,1);self.preview.hide()
        buttons=QHBoxLayout();layout.addLayout(buttons)
        self.back=QPushButton('Back');self.skip=QPushButton('Skip');self.use=QPushButton('Use proposal and Next')
        self.save_early=QPushButton('Save & apply choices');self.finish=QPushButton('Use reviewed plan');self.cancel=QPushButton('Cancel review')
        for button in (self.back,self.skip,self.use,self.save_early,self.finish,self.cancel):buttons.addWidget(button)
        self.back.clicked.connect(self.previous);self.skip.clicked.connect(lambda:self.advance(None))
        self.use.clicked.connect(lambda:self.advance(self.choice.currentData()))
        self.save_early.clicked.connect(self.save_early_and_accept)
        self.finish.clicked.connect(self.accept);self.cancel.clicked.connect(self.reject)
        self.choice.currentIndexChanged.connect(self.describe)
        self.show_step()

    def save_early_and_accept(self):
        if self.position<len(self.groups) and self.choice.count()>0:
            c=self.choice.currentData()
            if c is not None:self.decisions[self.position]=tuple(c)
        self.summary(force=True)
        self.accept()

    def show_step(self):
        summary=self.position==len(self.groups)
        self.preview.setVisible(summary);self.finish.setVisible(summary)
        for widget in (self.choice,self.details,self.skip,self.use,self.save_early):widget.setVisible(not summary)
        self.back.setEnabled(self.position>0)
        if summary:self.summary();return
        group=self.groups[self.position];tags=group[0].get('tags',{})
        self.heading.setText(f'Album {self.position+1} of {len(self.groups)} · {first(tags,"albumartist")} — {first(tags,"album")}')
        self.description.setText(f'{len(group)} local file(s). Choose the album artist used to group these files. Review every proposed tag below. Existing BPM and key stay protected. Organise files separately after saving.')
        self.choice.blockSignals(True);self.choice.clear()
        for option in group[0].get('catalogue_options',[]):
            self.choice.addItem(f"{option['artist']} — {option['album']} · {option.get('credit_label','Verified album credit')} · TIDAL {option['id']}",option_key(option))
        previous=self.decisions.get(self.position)
        if previous is not None:
            for i in range(self.choice.count()):
                if tuple(self.choice.itemData(i))==tuple(previous):self.choice.setCurrentIndex(i);break
        self.choice.blockSignals(False)
        has_any_choice=any(d is not None for d in self.decisions.values()) or (self.choice.count()>0)
        self.save_early.setEnabled(has_any_choice)
        self.use.setEnabled(self.choice.count()>0);self.choice.setEnabled(self.choice.count()>0);self.describe()

    def describe(self):
        if self.position>=len(self.groups):return
        key=self.choice.currentData();lines=[]
        for row in self.groups[self.position]:
            rel_path = str(Path(row['path']).relative_to(row['root']))
            lines.append(f"📁 Local File: {rel_path}")
            status = row.get('catalogue_note') or row.get('blocked') or 'No verified correction available.'
            lines.append(f"   Status: {status}")
            option=next((o for o in row.get('catalogue_options',[]) if key is not None and option_key(o)==tuple(key)),None)
            if option:
                lines.append(f"   🌐 Online TIDAL Match: {option['artist']} — {option['album']} (TIDAL Album {option['id']})")
                if option['changes']:
                    lines.append("   🏷 Tag Updates (Local [Saved] → Online [Proposed]):")
                    for tag,value in option['changes'].items():
                        local_val = ", ".join(row.get("tags",{}).get(tag,[])) or "(empty / missing)"
                        online_val = ", ".join(value)
                        lines.append(f"      • {tag}: {local_val} → {online_val}")
                else:
                    lines.append("   ✓ Local tags already agree with this online placement.")
            lines.append("")
        self.details.setPlainText('\n'.join(lines))

    def advance(self,choice):
        self.decisions[self.position]=tuple(choice) if choice is not None else None
        self.position+=1;self.show_step()

    def previous(self):
        self.position=max(0,self.position-1);self.show_step()

    def summary(self, force=False):
        if not force and self.position!=len(self.groups):return
        chosen=[]
        for i,group in enumerate(self.groups):
            key=self.decisions.get(i)
            if key is None:continue
            for row in copy.deepcopy(group):
                option=next((o for o in row.get('catalogue_options',[]) if option_key(o)==key),None)
                if not option:continue
                # A tag-agreement choice is not permission for unrelated date or path changes.
                # A placement can link the database even when tags already agree.
                for tag in option['changes']:row.get('overrides',{}).pop(tag,None)
                row['catalogue_choice']=option
                row['catalogue_note']=option['evidence']+' · placement chosen by you'
                chosen.append(row)
        self.plans=workflow_plan(chosen,'links',dates=False,discs=False)
        self.heading.setText(f'Review complete · {len(self.plans)} files in the repair plan')
        self.description.setText('Use reviewed plan returns to Link Releases with these files selected. Apply tag corrections writes them. File locations stay unchanged; organise them separately after saving the tags.')
        self.preview.setRowCount(len(self.plans))
        for i,row in enumerate(self.plans):
            values=(str(Path(row['path']).relative_to(row['root'])),
                    '; '.join(f'{k}: {", ".join(v)}' for k,v in row['changes'].items()))
            for j,value in enumerate(values):
                item=QTableWidgetItem(value);item.setToolTip(value);self.preview.setItem(i,j,item)
        self.finish.setText('Use reviewed plan' if self.plans else 'Finish — no changes')
