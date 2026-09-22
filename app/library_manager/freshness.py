"""Refresh only changed file contents before dependent catalogue operations."""
from .core import scan
from .library_workflows import inspect_snapshot
from .maintenance import first


def prepare_library(store,root,snapshot,cancel,progress):
    if not snapshot:snapshot=cached_inspection(store,root)
    progress('Checking for local changes before continuing · unchanged tags are reused')
    try:
        result=scan(store,root,cancelled=cancel,progress=progress)
    except OSError:
        if snapshot:
            progress('Library drive is offline · using cached snapshot')
            return list(snapshot)
        raise ValueError('Library drive is offline; reconnect the drive.')
    if cancel():
        return list(snapshot)
    if result.get('status')!='complete':
        if snapshot: return list(snapshot)
        raise ValueError('Local refresh did not complete; the previous preview is retained. Reconnect the drive or retry.')
    try:
        paths=[r['path'] for r in store.rows("SELECT path FROM local_files WHERE root=? AND present=1 AND lower(path) LIKE '%.flac'",(str(root),))]
        rows=inspect_snapshot(root,snapshot,cancel,progress,store.preferences('organisation'),paths=paths)
    except (OSError, FileNotFoundError):
        if snapshot:
            progress('Library drive is offline · using cached snapshot')
            return list(snapshot)
        raise
    if not cancel():save_inspection(store,root,rows)
    for row in rows:
        if first(row.get('tags',{}),'albumartist') in result.get('changed_artists', ()):row['needs_artist_match']=True
    return rows


INSPECTION_FIELDS=('path','root','stamp','tags','has_artwork','cover_size','duration','blocked')

def cached_inspection(store, root):
    data=store.preferences('inspection:'+str(root))
    rows=[]
    for source in data.get('rows',[]) if data.get('version')==1 else []:
        row=dict(source, target=source['path'],changes={},issues=[],organise=False)
        row['stamp']=tuple(row.get('stamp',()))
        if row.get('cover_size'):row['cover_size']=tuple(row['cover_size'])
        rows.append(row)
    return rows

def save_inspection(store, root, rows):
    # Persist observations only, never file-operation proposals or approvals.
    store.save_preferences('inspection:'+str(root),dict(version=1,rows=[
        {key:row[key] for key in INSPECTION_FIELDS if key in row} for row in rows]))
