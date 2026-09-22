"""One indexed, read-only definition of active track and local-release links."""
import json
from collections import defaultdict
from .linking import stamps_match
from .release_matching import group_key


def link_statistics(store, market, root=None):
    query='SELECT path,root,size,mtime,metadata FROM local_files WHERE present=1 AND metadata IS NOT NULL'
    records=store.rows(query+(' AND root=?' if root else ''),(str(root),) if root else ())
    saved={r['path']:r for r in store.rows('SELECT path,stamp,payload FROM track_links WHERE market=?',(market,))}
    active={};groups=defaultdict(list)
    for record in records:
        track=json.loads(record['metadata']);path=record['path']
        ids={}
        previous=saved.get(path)
        if previous:
            payload=json.loads(previous['payload'])
            if stamps_match(json.loads(previous['stamp']),(record['size'],record['mtime'])):ids=payload.get('ids') or {}
            # A durable unlink or changed file overrides historical ID tags.
        else:
            ids={'album_id':track.get('tidal_album_id'),'track_id':track.get('tidal_track_id')}
        if ids.get('album_id') and ids.get('track_id'):active[path]=ids
        groups[group_key(dict(track,path=path))].append(path)
    complete=sum(all(p in active for p in paths) for paths in groups.values())
    partial=sum(any(p in active for p in paths) and not all(p in active for p in paths) for paths in groups.values())
    return dict(track_count=len(records),linked_tracks=len(active),release_count=len(groups),
                linked_releases=complete,partial_releases=partial,active=active)


def summary(stats):
    return (f"{stats['linked_releases']:,} / {stats['release_count']:,} releases linked · "
            f"{stats['partial_releases']:,} partially linked · "
            f"{stats['linked_tracks']:,} / {stats['track_count']:,} tracks linked")


def needs_edition_choice(row):
    options=row.get('catalogue_options') or []
    note=' '.join(str(row.get(key) or '') for key in ('catalogue_note','blocked','link_note')).casefold()
    return bool(options) or any(term in note for term in ('too many','needs choice','ambiguous','multiple editions','positions differ','totals differ'))


def matches_link_filter(row, name, active, ignored):
    path=row.get('path');linked=path in active;excluded=path in ignored
    if name=='Ignored files':return excluded
    if name=='All files':return True
    if excluded:return False
    if name=='Linked tracks':return linked
    if name in ('Needs attention','Unlinked tracks'):return not linked
    if name=='Too many editions / Needs choice':return not linked and needs_edition_choice(row)
    return True


def unresolved_paths(store, market, rows, editions_only=False):
    """Candidate count is not link state: accepted equivalent editions stay linked."""
    active=link_statistics(store,market)['active']
    ignored=store.ignored_local_files()
    paths=set()
    for row in rows:
        path=row['path']
        if path in active or path in ignored:continue
        if editions_only and not needs_edition_choice(row):continue
        paths.add(path)
    return paths
