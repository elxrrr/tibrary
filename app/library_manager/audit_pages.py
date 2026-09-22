"""Small native pages for read-only audits and explicitly reviewed actions."""
from pathlib import Path
from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QWidget,QVBoxLayout,QHBoxLayout,QLabel,QPushButton,QComboBox,
                              QLineEdit,QTableWidget,QTableWidgetItem,QHeaderView,QAbstractItemView,QMessageBox,QMenu,QApplication)


class AuditPage(QWidget):
    def __init__(self, host, kind):
        super().__init__(); self.host=host; self.kind=kind; self.rows=[]
        layout=QVBoxLayout(self);layout.setContentsMargins(16,16,16,16);layout.setSpacing(12)
        title=QLabel('MQA Audit' if kind=='mqa' else ('Online Replacements' if kind=='online_optimizations' else 'Local Consolidation'));title.setObjectName('title');layout.addWidget(title)
        self.description=QLabel('Read-only inspection of indexed FLAC files. Results are reused until files change.' if kind=='mqa' else
                                ('Find larger online releases containing all your existing recordings. Download first, then review consolidation. Exclusive mixes are kept.' if kind=='online_optimizations' else 'Find smaller releases whose recordings are already in a complete local album. Review before moving redundant files to Trash.'))
        self.description.setWordWrap(True);layout.addWidget(self.description)
        bar=QHBoxLayout();self.roots=QComboBox();bar.addWidget(self.roots,1)
        self.run=QPushButton('Inspect library' if kind=='mqa' else 'Find optimizations');bar.addWidget(self.run);self.run.clicked.connect(lambda:self.inspect(online=self.kind=='online_optimizations'))
        self.recheck=QPushButton('Recheck files' if kind=='mqa' else 'Check candidate releases…');bar.addWidget(self.recheck);self.recheck.clicked.connect(lambda:self.inspect(force=kind=='mqa',online=kind!='mqa'));self.recheck.setVisible(kind!='optimizations');layout.addLayout(bar)
        filters=QHBoxLayout();self.search=QLineEdit();self.search.setPlaceholderText('Filter results…');filters.addWidget(self.search,1)
        self.filter=QComboBox();self.filter.addItems(['Signals and clues','All files','MQA signal','MQA tags','Not checked']);self.filter.setVisible(kind=='mqa');filters.addWidget(self.filter);layout.addLayout(filters)
        self.table=QTableWidget();self.table.setColumnCount(8 if kind=='mqa' else 10)
        self.table.setHorizontalHeaderLabels(['Artist','Release','Local File','Status','Encoded audio','Original rate','Evidence','Target Action'] if kind=='mqa' else ['Artist','Release','Local Release','Destination','Tracks gained','Duplicates','Space recovered','Status','Evidence','Target Action'])
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows);self.table.setSelectionMode(QAbstractItemView.SelectionMode.ExtendedSelection)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().hide();self.table.verticalHeader().setDefaultSectionSize(30)
        self.table.setWordWrap(False);self.table.setTextElideMode(Qt.TextElideMode.ElideMiddle)
        header=self.table.horizontalHeader();header.setStretchLastSection(False)
        header.setSectionResizeMode(QHeaderView.ResizeMode.Interactive)
        for col,width in enumerate([280,112,150,112,280] if kind=='mqa' else [280,280,110,110,130]):self.table.setColumnWidth(col,width)
        for col in ([1,2] if kind=='mqa' else [1,2]):header.setSectionResizeMode(col,QHeaderView.ResizeMode.Stretch)
        from .ui import QuietFocusDelegate
        self.table.setItemDelegate(QuietFocusDelegate(self.table))
        self.table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.table.customContextMenuRequested.connect(self.context_menu)
        self.table.setSortingEnabled(True);layout.addWidget(self.table,1)
        select_bar=QHBoxLayout();self.select_all=QPushButton('Select visible');self.select_all.clicked.connect(self.select_visible);select_bar.addWidget(self.select_all);clear=QPushButton('Clear selection');clear.clicked.connect(self.clear_checked);select_bar.addWidget(clear);self.selection_count=QLabel('0 selected');select_bar.addWidget(self.selection_count);select_bar.addStretch();layout.addLayout(select_bar)
        self.status=QLabel('Choose the library to inspect.');self.status.setWordWrap(True);layout.addWidget(self.status)
        actions=QHBoxLayout();actions.addStretch();self.action=QPushButton('Queue replacement…' if kind=='mqa' else 'Remove local duplicates…' if kind=='optimizations' else 'Consolidate release…');actions.addWidget(self.action);layout.addLayout(actions)
        self.roots.currentIndexChanged.connect(self.root_changed)
        self.action.clicked.connect(self.act);self.search.textChanged.connect(self.filter_rows);self.filter.currentTextChanged.connect(self.filter_rows)
        self.table.itemSelectionChanged.connect(self.update_action)
        self.update_action()

    def context_menu(self,pos):
        index=self.table.indexAt(pos)
        if not index.isValid():return
        if index.row() not in {r.row() for r in self.table.selectionModel().selectedRows()}:self.table.selectRow(index.row())
        from PySide6.QtCore import QItemSelectionModel
        self.table.selectionModel().setCurrentIndex(index,QItemSelectionModel.SelectionFlag.NoUpdate)
        source=self.rows[self.table.item(index.row(),0).data(Qt.ItemDataRole.UserRole)]
        path=source.get('path') or source.get('folder')
        from .context_actions import context_menu
        menu=context_menu(self,path=path,metadata=source)
        action=menu.addAction('Queue selected replacements…' if self.kind=='mqa' else ('Remove local duplicates…' if source.get('kind')=='local' else 'Consolidate release…'))
        action.triggered.connect(self.act)
        menu.exec(self.table.viewport().mapToGlobal(pos))

    def start_job(self,operation,done=None,**kwargs):
        from .tidal import CatalogueError
        from mutagen import MutagenError
        def checked(cancel,progress):
            try:return operation(cancel,progress)
            except (ValueError,OSError,RuntimeError,MutagenError) as exc:raise CatalogueError(str(exc)) from None
        self.status.setText(kwargs.get('label','Working')+'…')
        kwargs.setdefault('on_failure',self.status.setText)
        self.host.job(checked,done,**kwargs)
        if self.host.worker:
            self.host.worker.message.connect(self.status.setText,Qt.ConnectionType.QueuedConnection)
            self.host.worker.finished.connect(self.update_action,Qt.ConnectionType.QueuedConnection)

    def showEvent(self,event):
        super().showEvent(event)
        current=self.roots.currentData() or (self.host.tools_root.currentData() if hasattr(self.host,'tools_root') else None)
        self.roots.blockSignals(True);self.roots.clear()
        for row in self.host.store.rows('SELECT root FROM roots ORDER BY root'):self.roots.addItem(row['root'],row['root'])
        if current:self.roots.setCurrentIndex(max(0,self.roots.findData(current)))
        self.roots.blockSignals(False);self.root_changed()

    def load_library(self,root):
        self.roots.blockSignals(True)
        if self.roots.findData(root)<0:self.roots.addItem(root,root)
        self.roots.setCurrentIndex(self.roots.findData(root));self.roots.blockSignals(False)
        self.root_changed()

    def root_changed(self,*args):
        root=self.roots.currentData()
        if getattr(self,'result_root',None)==root:return
        self.result_root=root
        if root and self.kind=='mqa':
            from .mqa_audit import cached_audit
            self.show_results(cached_audit(self.host.store,root));return
        if root and self.kind!='mqa':
            from .optimizations import load_optimization_results
            cached=load_optimization_results(self.host.store,root,self.host.market,self.scope(),allow_stale=True)
            if cached is not None:
                self.show_results(cached);self.status.setText(f'{len(cached):,} saved opportunities · verified again before any file changes');return
        self.show_results([])
        self.status.setText('Choose Inspect library to read files.' if self.kind=='mqa' else 'Choose Find optimizations to compare cached releases.')

    def scope(self):return 'remote' if self.kind=='online_optimizations' else 'local'

    def inspect(self,checked=False,force=False,online=False):
        root=self.roots.currentData()
        if not root or self.host.worker:return
        if self.host.demo_mode:return self.host.demo_notice()
        def work(cancel,progress):
            if self.kind=='mqa':
                from .mqa_audit import audit_library
                return audit_library(self.host.store,root,cancel,progress,force)
            from .optimizations import find_optimizations,check_candidate_releases
            if online:
                check_candidate_releases(self.host.store,self.host.market,root,self.host.api(cancel,progress),cancel,progress)
            scope='remote' if self.kind=='online_optimizations' else 'local'
            return find_optimizations(self.host.store,self.host.market,root,cancel,progress,scope=scope)
        def done(rows):
            if self.host.worker and self.host.worker.isInterruptionRequested():
                self.status.setText('Cancelled · previous results retained');return
            if self.kind!='mqa':
                from .optimizations import save_optimization_results
                save_optimization_results(self.host.store,root,self.host.market,self.scope(),rows)
            if self.roots.currentData()==root:
                self.result_root=root;self.show_results(rows)
        self.start_job(work,done,label='MQA audit' if self.kind=='mqa' else 'Compare release optimizations',local=not online)

    def show_results(self,rows):
        self.rows=rows;self.table.blockSignals(True);self.table.setSortingEnabled(False);self.table.setRowCount(len(rows))
        indexed={t['path']:t for t in self.host.store.tracks()} if self.kind=='mqa' else {}
        from .link_statistics import link_statistics
        linked=link_statistics(self.host.store,self.host.market)['active'] if self.kind=='mqa' and rows else {}
        for n,row in enumerate(rows):
            if self.kind=='mqa':
                values=[row['path'],row['status'],f"{row.get('bits','?')}-bit / {row.get('rate','?')} Hz",f"{row['original_rate']} Hz" if row.get('original_rate') else 'Unknown',row['evidence'],('Online track '+str(linked[row['path']]['track_id'])+' · release '+str(linked[row['path']]['album_id'])+' · original retained' if row['path'] in linked else 'Link recording before choosing replacement; no target selected'),indexed.get(row['path'],{}).get('artist','—'),indexed.get(row['path'],{}).get('album','—'),'FLAC',indexed.get(row['path'],{}).get('track','—')]
            else:
                destination=row.get('target_folder') or row['release']['title']
                values=[row['folder'],destination,row['gained'],row['duplicates'],f"{row['recoverable_bytes']/1024**2:.1f} MB",'Verified subset',f"All {row['duplicates']} source recordings match; exclusive recordings must be retained",f"Verify {destination}; then review moving {row['folder']} to Trash",row['release'].get('artist','—'),row['release'].get('title','—')]
            full_path=str(values[0])
            try:values[0]=str(Path(full_path).relative_to(self.roots.currentData()))
            except (ValueError,TypeError):values[0]=Path(full_path).name
            action_detail=str(values[5] if self.kind=='mqa' else values[7])
            if self.kind=='mqa':
                values[5]='Replace stream' if row['path'] in linked else 'Link recording'
                values=[values[i] for i in (6,7,0,1,2,3,4,5)]
            else:
                values[7]='Remove local duplicates' if row.get('kind')=='local' else 'Download replacement'
                values=[values[i] for i in (8,9,0,1,2,3,4,5,6,7)]
            for c,value in enumerate(values):
                from .ui import SortableTableItem
                item=SortableTableItem(str(value));item.setData(Qt.ItemDataRole.UserRole,n);item.setToolTip(full_path if c==2 else action_detail if c==self.table.columnCount()-1 else str(value));self.table.setItem(n,c,item)
        if hasattr(self.host,'health_audit_cards'):
            card=self.host.health_audit_cards.get('mqa' if self.kind=='mqa' else 'local') if self.kind!='online_optimizations' else getattr(self.host,'remote_health_card',None)
            if card:
                count=sum(bool(r.get('detected')) for r in rows) if self.kind=='mqa' else len(rows)
                title={'mqa':'🔬  MQA Audit','optimizations':'♻️  Local Consolidation','online_optimizations':'♻️  Online Replacements'}[self.kind]
                card.setText(f'{title}\n{count:,} '+('flagged files' if self.kind=='mqa' else 'opportunities'))
                if self.kind=='mqa':card.description_label.setText(f"{len(rows)-sum(r['status']=='Not checked' for r in rows):,} inspected · {sum(r['status']=='Not checked' for r in rows):,} awaiting inspection")
        self.table.blockSignals(False);self.table.setSortingEnabled(True);self.status.setText(f'{len(rows):,} results · local audio unchanged');self.filter_rows();self.update_action()

    def queue_replacement(self,row):
        from .release_matching import recording_matches
        preserve={}
        for source in row['sources']:
            for track in row['release'].get('tracks',[]):
                if recording_matches(source,track,strict=True):
                    preserve[str(track['id'])]={k:[source[v]] for k,v in [('bpm','bpm'),('initialkey','musical_key')] if source.get(v)}
        self.host.store.enqueue(dict(row['release'],preserve_local_dj=preserve))

    def select_visible(self):
        from PySide6.QtCore import QItemSelection, QItemSelectionModel
        selection=QItemSelection()
        for n in range(self.table.rowCount()):
            if not self.table.isRowHidden(n):
                selection.select(self.table.model().index(n,0),self.table.model().index(n,self.table.columnCount()-1))
        self.table.selectionModel().select(selection,QItemSelectionModel.SelectionFlag.ClearAndSelect)

    def clear_checked(self):
        self.table.clearSelection()

    def update_action(self,*args):
        idx=self.table.currentRow()
        count=len(self.selected_results())
        self.selection_count.setText(f'{count:,} selected')
        self.action.setEnabled(bool(count) and self.host.worker is None)
        if self.kind=='mqa':return
        if idx<0 or not self.table.item(idx,0):
            self.action.setText('Remove local duplicates…' if self.kind=='optimizations' else 'Consolidate release…');return
        row=self.rows[self.table.item(idx,0).data(Qt.ItemDataRole.UserRole)]
        self.action.setText('Remove local duplicates…' if row.get('kind')=='local' else 'Consolidate release…')

    def filter_rows(self,*args):
        query=self.search.text().casefold();choice=self.filter.currentText()
        for n in range(self.table.rowCount()):
            row=self.rows[self.table.item(n,0).data(Qt.ItemDataRole.UserRole)]
            hidden=query not in (' '.join(self.table.item(n,c).text() for c in range(self.table.columnCount()))+' '+str(row.get('path') or row.get('folder'))).casefold()
            if self.kind=='mqa':hidden |= (choice=='Signals and clues' and row['status']=='No signal found') or (choice not in ('Signals and clues','All files') and row['status']!=choice)
            self.table.setRowHidden(n,hidden)
        self.update_action()

    def selected_results(self):
        indices={i.row() for i in self.table.selectionModel().selectedRows()}
        return [self.rows[self.table.item(n,0).data(Qt.ItemDataRole.UserRole)] for n in sorted(indices) if not self.table.isRowHidden(n)]

    def review_dj_differences(self,row,files):
        from .optimizations import dj_tag_conflicts
        from PySide6.QtWidgets import QDialog,QDialogButtonBox
        conflicts=dj_tag_conflicts(row,files)
        if not conflicts:return True
        dialog=QDialog(self);dialog.setWindowTitle('Review duplicate tag differences');dialog.resize(1100,520)
        layout=QVBoxLayout(dialog)
        note=QLabel('These recordings match, but their BPM or key differs. The smaller release will move to Trash. The retained files and their tags will stay unchanged. Cancel to correct or preserve tags first.')
        note.setWordWrap(True);layout.addWidget(note)
        table=QTableWidget(len(conflicts),5)
        table.setHorizontalHeaderLabels(['File to remove','File to keep','Tag','Value removed','Value kept'])
        table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers);table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        for n,c in enumerate(conflicts):
            for col,key in enumerate(('source','replacement','field','removed','retained')):
                text=c[key] if key not in ('source','replacement') else Path(c[key]).name
                if key=='field':text='BPM' if text=='bpm' else 'Musical key'
                item=QTableWidgetItem(text);item.setToolTip(c[key]);table.setItem(n,col,item)
        table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.Stretch);layout.addWidget(table)
        buttons=QDialogButtonBox(QDialogButtonBox.StandardButton.Cancel)
        proceed=buttons.addButton('Keep replacement tags and continue',QDialogButtonBox.ButtonRole.AcceptRole)
        buttons.accepted.connect(dialog.accept);buttons.rejected.connect(dialog.reject);layout.addWidget(buttons)
        if dialog.exec()!=QDialog.DialogCode.Accepted:return False
        row['reviewed_dj_conflicts']=conflicts
        return True

    def act_many(self,rows):
        if self.host.demo_mode:return self.host.demo_notice()
        from .optimizations import replacement_files,validate_consolidation,consolidate,save_optimization_results
        ready_rows=[];download_rows=[];verified_files={}
        def verify(cancel,progress):
            for row in rows:
                if cancel():raise ValueError('Cancelled; no files changed')
                if row.get('kind')!='local' and not replacement_files(self.host.store,row):download_rows.append(row)
                else:
                    verified_files[row['folder']]=validate_consolidation(self.host.store,row,cancel=cancel,progress=progress,review_dj=True);ready_rows.append(row)
            sources={str(Path(row['folder']).resolve()) for row in ready_rows}
            if any(str(Path(row.get('target_folder','')).resolve()) in sources for row in ready_rows):
                raise ValueError('A selected destination is also a source. Consolidate these releases separately.')
            return True
        def ready(_):
            def confirm():
                for row in ready_rows:
                    if not self.review_dj_differences(row,verified_files[row['folder']]):return
                descriptions=[f"{row['folder']} → {row.get('target_folder') or row['release']['title']}" for row in ready_rows]
                descriptions += [f"Queue complete replacement: {row['release']['title']} (original retained)" for row in download_rows]
                if QMessageBox.question(self,'Review selected operations',f'{len(ready_rows)} releases to consolidate to Trash; {len(download_rows)} replacement releases to queue.\n\n'+'\n'.join(descriptions))!=QMessageBox.StandardButton.Yes:return
                def execute(cancel,progress):
                    completed=[]
                    for row in ready_rows:
                        if cancel():break
                        consolidate(self.host.store,self.host.market,row,cancel=cancel);completed.append(row['folder'])
                    if not cancel():
                        for row in download_rows:self.queue_replacement(row)
                    return completed
                def done(completed):
                    remaining=[r for r in self.rows if r['folder'] not in completed]
                    root=self.result_root
                    self.host._library_content_changed(root)
                    save_optimization_results(self.host.store,root,self.host.market,self.scope(),remaining)
                    self.show_results(remaining);self.host.refresh()
                    self.status.setText(f'{len(completed)} releases consolidated; replacement downloads require queue approval')
                self.start_job(execute,done,label='Apply reviewed consolidations',local=True,is_disk_op=True)
            self.host._after_job=confirm
        self.start_job(verify,ready,label='Verify selected replacements',local=True)

    def act(self):
        idx=self.table.currentRow()
        selected=self.selected_results()
        if self.host.worker:
            self.status.setText('Another operation is running. Wait for it to finish or cancel it in Activity.');return
        if selected:
            if self.kind=='mqa':
                if self.host.demo_mode:return self.host.demo_notice()
                self.queue_mqa(selected);return
            if len(selected)>1:self.act_many(selected);return
        if selected:row=selected[0]
        elif idx<0 or self.table.isRowHidden(idx):
            self.status.setText('Select a visible release first.');return
        else:row=self.rows[self.table.item(idx,0).data(Qt.ItemDataRole.UserRole)]
        if self.host.demo_mode:return self.host.demo_notice()
        if self.kind=='mqa':
            selected=[self.rows[self.table.item(i.row(),0).data(Qt.ItemDataRole.UserRole)] for i in self.table.selectionModel().selectedRows() if not self.table.isRowHidden(i.row())]
            self.queue_mqa(selected or [row]);return
        from .optimizations import replacement_files,validate_consolidation,consolidate
        if row.get('kind')!='local' and not replacement_files(self.host.store,row):
            if QMessageBox.question(self,'Download replacement release',f"Queue the complete {row['release']['title']} release?\nThe smaller release stays in place. After downloading, return here to review consolidation.")!=QMessageBox.StandardButton.Yes:return
            self.queue_replacement(row);self.host.refresh();self.status.setText('Complete replacement queued. Approve and download, then return to consolidate.');return
        def ready(files):
            def confirm():
                if not self.review_dj_differences(row,files):return
                if row.get('kind')=='local':
                    message=f"The complete local album has been verified at:\n{row['target_folder']}\n\nMove the redundant smaller release to macOS Trash?\n{row['folder']}\n\n{row['duplicates']} duplicate files · {row['recoverable_bytes']/1024**2:.1f} MB recoverable (Trash still uses space until emptied)."
                    title='Remove local duplicates'
                else:
                    message=f"All {len(files)} replacement tracks have been verified.\nMove this smaller release folder to macOS Trash?\n\n{row['folder']}\n\n{row['duplicates']} redundant files · {row['recoverable_bytes']/1024**2:.1f} MB recoverable (Trash still uses space until emptied)."
                    title='Review consolidation'
                if QMessageBox.question(self,title,message)!=QMessageBox.StandardButton.Yes:return
                self.start_job(lambda cancel,progress:consolidate(self.host.store,self.host.market,row,cancel=cancel),
                              lambda count:self.consolidated(row,count),label='Consolidate release',local=True,is_disk_op=True)
            self.host._after_job=confirm
        self.start_job(lambda cancel,progress:validate_consolidation(self.host.store,row,cancel=cancel,progress=progress,review_dj=True),ready,label='Verify complete replacement',local=True)

    def consolidated(self,row,count):
        remaining=[r for r in self.rows if r['folder']!=row['folder']]
        self.host.tools_snapshots.pop(row['root'],None)
        self.host._dirty_roots.add(row['root'])
        self.host.tools_plan=[];self.host.invalidate_tools_plan();self.host._library_content_changed(row['root'])
        from .optimizations import save_optimization_results
        save_optimization_results(self.host.store,row['root'],self.host.market,self.scope(),remaining)
        self.result_root=row['root'];self.show_results(remaining)
        self.status.setText(f'{count} redundant files moved to Trash')

    def queue_mqa(self,rows):
        if isinstance(rows,dict):rows=[rows]
        rows=[r for r in rows if r.get('detected')]
        if not rows:
            QMessageBox.information(self,'No confirmed MQA evidence','Select MQA signal or MQA-tagged tracks to queue replacements.');return
        from .linking import attach_links
        from .maintenance import inspect_file,first
        root=self.roots.currentData()
        def work(cancel,progress):
            import time
            from .release_matching import recording_matches
            releases={};selections={};preserve={}
            for n,row in enumerate(rows):
                if cancel():raise ValueError('Replacement search cancelled; queue unchanged')
                progress(f'Finding linked replacements · {n+1}/{len(rows)}')
                linked=attach_links([inspect_file(Path(row['path']),Path(root))],self.host.store,self.host.market)[0]
                if linked.get('blocked'):raise ValueError(linked['blocked'])
                if row.get('stamp') and list(linked['stamp'])!=row['stamp']:raise ValueError('A file changed since the MQA audit; inspect again')
                ids=linked.get('linked_ids',{})
                if not ids:
                    tags=linked['tags'];ids=dict(album_id=first(tags,'tidal_album_id'),track_id=first(tags,'tidal_track_id'))
                if not all(ids.get(k) for k in ('album_id','track_id')):raise ValueError('Link these tracks in Link Catalogue → Link Releases first')
                ident=str(ids['album_id'])
                if ident not in releases:
                    cached=self.host.store.preferences(f'tag-review:{self.host.market}:{ident}')
                    release=cached if cached.get('tracks_loaded') and time.time()-cached.get('tag_checked_at',0)<86400 else self.host.api(cancel,progress).album_tag_details({'id':ident})
                    releases[ident]=release
                    self.host.store.save_preferences(f'tag-review:{self.host.market}:{ident}',dict(release,tag_checked_at=time.time(),metadata_schema=3))
                release=releases[ident]
                selected=[t for t in release.get('tracks',[]) if str(t['id'])==str(ids['track_id'])]
                if len(selected)!=1 or not recording_matches(linked,selected[0],strict=True):raise ValueError('A saved link no longer agrees with its recording; recheck it in Link Releases')
                if release.get('available') is not True:raise ValueError('Replacement availability is unconfirmed')
                selections.setdefault(ident,{})[str(ids['track_id'])]=selected[0]
                tags=linked['tags']
                preserve.setdefault(ident,{})[str(ids['track_id'])]={k:[v] for k,v in [('bpm',first(tags,'bpm') or first(tags,'tempo')),('initialkey',first(tags,'initialkey') or first(tags,'key'))] if v}
            return releases,selections,preserve
        def ready(result):
            releases,selections,preserve=result
            prefs=self.host.store.preferences('downloads');quality=prefs.get('quality','Lossless')
            if quality not in ('Lossless','Hi-res lossless'):
                QMessageBox.information(self,'Choose lossless audio','Choose Lossless or Hi-res lossless in Settings → Downloads before queueing MQA replacements.');return
            for ident,release in releases.items():
                self.host.store.enqueue(dict(release,preserve_local_dj=preserve[ident],replacement_audit=dict(reason='MQA',quality=quality)),list(selections[ident].values()))
            self.host.refresh();self.status.setText(f'{sum(len(t) for t in selections.values())} replacements queued for approval. Use a separate download location; originals are never overwritten.')
        self.start_job(work,ready,label='Find linked MQA replacements')
