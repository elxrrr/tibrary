"""Read-only recording discovery and explicitly reviewed, guarded repairs."""
import copy
import json
import time
from pathlib import Path
from .core import title_key
from .maintenance import first, inspect_file, destination, validate_targets, apply_one
from .tidal import CatalogueError


def value(track, field):
    v=track.get(field, '')
    return str(v[0] if isinstance(v, (list,tuple)) and v else v or '')


def recording_match(local, remote):
    from .release_matching import recording_matches
    return recording_matches(dict(local,duration=local.get('duration') or local.get('duration_sec')),remote)


def discovery_for(release, local_tracks):
    credits=release.get('album_artists') or []
    if not credits or release.get('credits_complete') is False:return None
    matched=[]
    for local in local_tracks:
        matches=[t for t in release.get('tracks',[]) if t.get('id') and recording_match(local,t)]
        if len(matches)==1:
            matched.append(dict(local_track=copy.deepcopy(local),online_track_id=str(matches[0]['id']),online_track=copy.deepcopy(matches[0])))
    if not matched:return None
    artist=', '.join(dict.fromkeys(credits))
    return dict(release_id=str(release['id']),title=release.get('title',''),discovered_artist=artist,
                year=release.get('date','')[:4],date=release.get('date',''),release=copy.deepcopy(release),
                evidence=f'{len(matched)} recording(s) verified by ISRC, saved ID or exact title and duration',tracks_matched=matched)


def discover(local_tracks,store,market,api,cancel=lambda:False,progress=lambda _:None,query=None):
    cache={};out={};checked=set()
    for row in store.rows('SELECT payload FROM catalogue WHERE market=?',(market,)):
        for release in json.loads(row['payload']).get('releases',[]):cache[str(release['id'])]=release
    for row in store.rows('SELECT payload FROM app_preferences WHERE key LIKE ?',(f'tag-review:{market}:%',)):
        release=json.loads(row['payload']);cache[str(release['id'])]=release
    def inspect(release):
        ident=str(release['id'])
        if cancel() or ident in checked:return
        checked.add(ident)
        detail=cache.get(ident,release)
        if not (detail.get('tracks') and detail.get('album_artists') and time.time()-detail.get('tag_checked_at',0)<86400):
            try:detail=api.album_tag_details(release)
            except CatalogueError as exc:
                if exc.status not in (404,410):raise
                return
            detail=dict(detail,tag_checked_at=time.time(),metadata_schema=3)
            store.save_preferences(f'tag-review:{market}:{ident}',detail);cache[ident]=detail
        found=discovery_for(detail,local_tracks)
        if found:out[ident]=found
    if query:
        import re
        ident=re.fullmatch(r'(?:https?://(?:www\.)?tidal.com/(?:browse/)?album/)?(\d+)(?:/[^?]*)?(?:\?.*)?',query.strip())
        candidates=[dict(id=ident[1])] if ident else api.search_albums(query)
        for release in candidates[:12]:inspect(release)
        if not out and not ident:
            for track in api.search_tracks(query)[:8]:
                for aid in track.get('album_ids',[])[:3]:inspect(dict(id=str(aid)))
    else:
        # Only inspect locally plausible releases, not every cached artist's catalogue.
        local_isrcs={value(t,'isrc').replace('-','').upper() for t in local_tracks}-{''}
        local_ids={value(t,'tidal_track_id') for t in local_tracks}-{''}
        local_titles={title_key(value(t,'title')) for t in local_tracks}-{''}
        for release in list(cache.values()):
            if cancel():break
            if any(value(t,'isrc').replace('-','').upper() in local_isrcs or str(t.get('id')) in local_ids or title_key(value(t,'title')) in local_titles for t in release.get('tracks',[])):
                inspect(release)
        matched={m['local_track']['path'] for d in out.values() for m in d['tracks_matched']}
        lookups=set()
        for local in local_tracks:
            if cancel():break
            if local['path'] in matched:continue
            progress(f'Searching for: {value(local,"title") or Path(local["path"]).stem}')
            isrc=value(local,'isrc').replace('-','').upper();tid=value(local,'tidal_track_id')
            key=('isrc',isrc) if isrc else ('track_id',tid)
            if (isrc or tid) and key not in lookups:
                lookups.add(key)
                for release in api.recording_releases(**{key[0]:key[1]}):inspect(release)
            if any(m['local_track']['path']==local['path'] for d in out.values() for m in d['tracks_matched']):continue
            title=value(local,'title')
            key=('title',title)
            if title and key not in lookups:
                lookups.add(key)
                for release in api.search_albums(title)[:6]:inspect(release)
                for track in api.search_tracks(title)[:4]:
                    for aid in track.get('album_ids',[])[:2]:inspect(dict(id=str(aid)))
    return list(out.values())


def repair_preview(store,discovery,cancel=lambda:False):
    release=discovery.get('release')
    if not release:raise ValueError('Search again to verify this release before repairing files.')
    plans=[];seen=set();layout=store.preferences('organisation')
    for item in discovery.get('tracks_matched',[]):
        if cancel():break
        path=str(Path(item['local_track']['path']).resolve())
        if path in seen:continue
        seen.add(path)
        records=store.rows('SELECT root FROM local_files WHERE path=? AND present=1',(path,))
        if not records:raise ValueError('This file is no longer in the library. Refresh the library before repairing.')
        row=inspect_file(path,records[0]['root'],layout)
        if row['blocked']:raise ValueError(row['blocked'])
        local={k:first(row['tags'],k) for k in ('isrc','title','tidal_track_id')};local['duration']=row['duration']
        selected=[t for t in release.get('tracks',[]) if str(t.get('id'))==str(item.get('online_track_id')) and recording_match(local,t)]
        if len(selected)!=1:raise ValueError('Recording evidence changed or is ambiguous. Search again before repairing.')
        track=selected[0];credits=release.get('album_artists',[])
        if not credits or release.get('credits_complete') is False:raise ValueError('Complete album artist credits are required.')
        changes=dict(albumartist=[', '.join(dict.fromkeys(credits))],album=[release['title']],tidal_album_id=[str(release['id'])],tidal_track_id=[str(track['id'])])
        if release.get('date'):changes['date']=[release['date']]
        if track.get('title'):changes['title']=[track['title']]
        for field,tag in (('track_number','tracknumber'),('disc_number','discnumber')):
            if track.get(field):changes[tag]=[str(track[field])]
        if release.get('disc_count'):changes['disctotal']=[str(release['disc_count'])]
        count=sum(t.get('disc_number')==track.get('disc_number') for t in release.get('tracks',[]))
        if track.get('disc_number') and count:changes['tracktotal']=[str(count)]
        row['changes']={k:v for k,v in changes.items() if row['tags'].get(k)!=v}
        row['target']=str(destination(row['root'],dict(row['tags'],**changes),current_path=path,layout=layout))
        row['catalogue_choice']=dict(id=str(release['id']),track_id=str(track['id']),artist=changes['albumartist'][0],album=release['title'],changes={},recording_verified=True,evidence=discovery.get('evidence','Reviewed recording'))
        row['repair_artist_ids']=[str(i) for i in release.get('album_artist_ids',[]) if str(i).isdecimal()]
        plans.append(row)
    validate_targets(plans)
    if any(p.get('collision') for p in plans):raise ValueError('A destination already exists or two files share a destination. Resolve this before applying.')
    if not plans:raise ValueError('No verified files to repair.')
    return plans


def apply_repairs(store,market,plans,cancel=lambda:False,progress=lambda _:None):
    from .linking import save_result
    applied=0
    for row in plans:
        if cancel():break
        progress(f'Repairing {applied+1}/{len(plans)} · {Path(row["path"]).name}')
        apply_one(row,store)
        fresh=inspect_file(row['target'],row['root'])
        fresh['catalogue_choice']=row['catalogue_choice']
        if not save_result(store,market,fresh,manual=True):raise ValueError('File updated but link could not be saved. Refresh this file.')
        if row['path']!=row['target']:
            with store.connect() as db:db.execute('DELETE FROM track_links WHERE path=?',(row['path'],))
        ids=row.get('repair_artist_ids',[])
        if ids:
            artist=row['catalogue_choice']['artist']
            existing=[str(m['tidal_id']) for m in store.linked_mappings() if m['artist']==artist]
            combined=list(dict.fromkeys(existing+ids))
            store.mapping(artist,combined[0],'confirmed','Reviewed release repair',True,extra_ids=combined[1:])
        applied+=1
    return applied
