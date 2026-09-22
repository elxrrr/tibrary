"""Conservative supersede plans and a separately approved, guarded Trash step."""
import json
from collections import defaultdict
from pathlib import Path
from .core import read_metadata, now, norm
from .maintenance import fingerprint, guarded_path, inspect_file, trash_file
from .release_matching import group_key, release_folder, recording_matches, replacement_matches, total, value, position
from .view_data import release_is_out


def complete_group(tracks):
    if not tracks or not all(t.get('isrc') and t.get('duration') for t in tracks):return False
    discs=defaultdict(list)
    for track in tracks:discs[position(track)[0]].append(track)
    for items in discs.values():
        declared={total(t,'track') for t in items if total(t,'track')}
        expected=next(iter(declared)) if len(declared)==1 else 0
        if not expected or {position(t)[1] for t in items}!=set(range(1,expected+1)):return False
    expected_discs={total(t,'disc') for t in tracks if total(t,'disc')}
    return not expected_discs or (len(expected_discs)==1 and max(expected_discs)==len(discs))


def local_release(tracks):
    album_ids={str(t.get('tidal_album_id') or '') for t in tracks}-{''}
    ident=next(iter(album_ids)) if len(album_ids)==1 else 'local:'+str(abs(hash(str(release_folder(tracks[0])))))
    remote=[]
    for n,track in enumerate(sorted(tracks,key=position),1):
        remote.append(dict(id=str(track.get('tidal_track_id') or f'local:{n}'),title=track.get('title',''),
                           track_artist=track.get('track_artist',''),
                           isrc=track.get('isrc',''),duration=track.get('duration'),
                           track_number=position(track)[1],volume_number=position(track)[0]))
    return dict(id=ident,title=tracks[0].get('album') or release_folder(tracks[0]).name,
                artist=tracks[0].get('artist',''),date=max((t.get('date','') for t in tracks),default=''),
                type=tracks[0].get('releasetype') or 'ALBUM',available=True,tracks_loaded=True,
                track_count=len(remote),tracks=remote)


def local_optimization_plans(groups,root,cancel=lambda:False):
    """Find redundant complete releases using local audio before online data."""
    complete=[tracks for tracks in groups.values() if complete_group(tracks)]
    plans=[]
    for source in complete:
        if cancel():break
        source_folder=release_folder(source[0]);artist=norm(source[0].get('artist',''))
        matches=[]
        for target in complete:
            target_folder=release_folder(target[0])
            if target is source or target_folder==source_folder or norm(target[0].get('artist',''))!=artist or len(target)<=len(source):continue
            used=set();ok=True
            for track in source:
                hits=[t for t in target if t['path'] not in used and replacement_matches(track,t)]
                if len(hits)!=1:ok=False;break
                used.add(hits[0]['path'])
            if ok:matches.append(target)
        if not matches:continue
        # Prefer the smallest complete superset; it removes duplicates while
        # avoiding a leap to an unrelated compilation containing the track.
        target=min(matches,key=lambda rows:(len(rows),max((t.get('date','') for t in rows),default='')))
        try:
            guarded_path(source_folder,Path(root));guarded_path(release_folder(target[0]),Path(root))
            stamps={t['path']:list(fingerprint(Path(t['path']))) for t in source}
            if any((stamps[t['path']][2],stamps[t['path']][3])!=(t['size'],t['indexed_mtime']) for t in source):continue
        except (ValueError,OSError):continue
        release=local_release(target)
        plans.append(dict(kind='local',folder=str(source_folder),target_folder=str(release_folder(target[0])),
                          replacement_paths=[t['path'] for t in target],root=str(root),release=release,
                          sources=source,stamps=stamps,gained=len(target)-len(source),duplicates=len(source),
                          recoverable_bytes=sum(t.get('size') or 0 for t in source)))
    return plans


def find_optimizations(store, market, root, cancel=lambda:False, progress=lambda s:None, scope='all'):
    if scope not in ('all','local','remote'):raise ValueError('Unknown optimization scope')
    groups = defaultdict(list)
    for row in store.rows('SELECT * FROM local_files WHERE root=? AND present=1 AND error IS NULL', (str(root),)):
        if not row['metadata']: continue
        track = dict(json.loads(row['metadata']), path=row['path'], size=row['size'], indexed_mtime=row['mtime'])
        groups[group_key(track)].append(track)
    plans = local_optimization_plans(groups,root,cancel)
    if scope=='local':return plans
    candidates = {}
    for row in store.rows('SELECT payload FROM catalogue WHERE market=?', (market,)):
        for r in json.loads(row['payload']).get('releases', []): candidates[str(r['id'])] = r
    for row in store.rows('SELECT payload FROM app_preferences WHERE key LIKE ?', (f'tag-review:{market}:%',)):
        r = json.loads(row['payload']); candidates[str(r['id'])] = r
    ids_by_artist = defaultdict(set)
    for m in store.linked_mappings(): ids_by_artist[m['artist']].add(str(m['tidal_id']))
    release_ids = defaultdict(set)
    for c in store.rows('SELECT artist_id,payload FROM catalogue WHERE market=?', (market,)):
        for r in json.loads(c['payload']).get('releases', []): release_ids[c['artist_id']].add(str(r['id']))
    locally_resolved={p['folder'] for p in plans}
    if scope=='remote':plans=[]
    if plans:progress(f'Found {len(plans)} local consolidation opportunities before checking cached online releases')
    local_by_count=defaultdict(list)
    for tracks in groups.values():
        if complete_group(tracks):local_by_count[len(tracks)].append(tracks)
    downloaded_equivalents={}
    for n, tracks in enumerate(groups.values()):
        if cancel(): break
        if scope=='all' and str(release_folder(tracks[0])) in locally_resolved:continue
        allowed = set().union(*(release_ids[i] for i in ids_by_artist[tracks[0].get('artist','')]))
        # Unknown/missing source metadata must not turn into permission to prune.
        if not complete_group(tracks):continue
        for ident in allowed:
            r = candidates.get(ident, {})
            remote = r.get('tracks', [])
            if r.get('available') is not True or not r.get('tracks_loaded') or not release_is_out(r) or len(remote)<=len(tracks): continue
            if r.get('track_count') and int(r['track_count'])!=len(remote): continue
            if scope=='remote':
                if ident not in downloaded_equivalents:
                    downloaded_equivalents[ident]=any(
                        all(sum(replacement_matches(t,rt) for rt in remote)==1 for t in local)
                        for local in local_by_count.get(len(remote),[]))
                if downloaded_equivalents[ident]:continue

            local_date = max((t.get('date','') for t in tracks), default='')
            if local_date and r.get('date','') < local_date: continue
            matched = []
            for t in tracks:
                hits = [rt for rt in remote if replacement_matches(t,rt)]
                if len(hits)!=1: break
                matched.append(str(hits[0]['id']))
            if len(matched)!=len(tracks) or len(set(matched))!=len(matched): continue
            folder = release_folder(tracks[0])
            try:
                guarded_path(folder, Path(root))
                stamps = {t['path']: list(fingerprint(Path(t['path']))) for t in tracks}
                if any((stamps[t['path']][2],stamps[t['path']][3])!=(t['size'],t['indexed_mtime']) for t in tracks):continue
            except (ValueError, OSError): continue
            plans.append(dict(kind='remote',folder=str(folder), root=str(root), release=r, sources=tracks, stamps=stamps,
                              gained=len(remote)-len(tracks), duplicates=len(tracks),
                              recoverable_bytes=sum(t.get('size') or 0 for t in tracks)))
        if n%50==0: progress(f'Comparing cached releases · {n+1}/{len(groups)} local releases')
    return plans


def replacement_files(store, plan):
    if plan.get('replacement_paths'):
        return list(dict.fromkeys(plan['replacement_paths']))
    ident = str(plan['release']['id'])
    records = store.rows("SELECT payload FROM queue WHERE id=? AND decision='downloaded'", (ident,))
    files = json.loads(records[0]['payload']).get('downloaded_files', []) if records else []
    for t in store.tracks():
        if str(t.get('tidal_album_id'))==ident: files.append(t['path'])
    return list(dict.fromkeys(p for p in files if Path(p).suffix.lower()=='.flac' and not Path(p).is_relative_to(Path(plan['folder']))))


def dj_tag_conflicts(plan, inspected):
    from .musical_keys import camelot_key
    def comparable(field,value):
        if field=='musical_key':return camelot_key(value) or value
        try:return float(value)
        except (TypeError,ValueError):return value
    conflicts=[]
    for source in plan['sources']:
        hits=[t for t in inspected if replacement_matches(source,t)]
        if len(hits)!=1:raise ValueError('An exclusive or ambiguous recording would be lost')
        for field in ('bpm','musical_key'):
            if source.get(field) and comparable(field,source[field])!=comparable(field,hits[0].get(field)):
                conflicts.append(dict(source=source['path'],replacement=hits[0]['path'],field=field,removed=str(source[field]),retained=str(hits[0].get(field) or 'Missing')))
    return conflicts


def validate_consolidation(store, plan, decode=True, cancel=lambda:False, progress=lambda _:None, review_dj=False):
    folder, root = Path(plan['folder']), Path(plan['root'])
    guarded_path(folder,root)
    if not folder.is_dir(): raise ValueError('Source folder is no longer present; refresh the comparison')
    progress('Checking source recordings and saved tags…')
    for path, stamp in plan['stamps'].items():
        if cancel():raise ValueError('Consolidation cancelled; original files retained')
        guarded_path(Path(path),root)
        if list(fingerprint(Path(path)))!=stamp: raise ValueError('Source files changed; refresh the comparison')
        original=next(t for t in plan['sources'] if t['path']==path)
        if not replacement_matches(original,read_metadata(Path(path))):raise ValueError('Source recording or performer tags changed; rescan before consolidation')
    allowed = set(plan['stamps'])
    for path in folder.rglob('*'):
        if path.is_symlink(): raise ValueError('Source folder contains a symbolic link; it cannot be consolidated')
        if path.is_file() and str(path) not in allowed and path.name.lower() not in ('cover.jpg','cover.png','folder.jpg','.ds_store'):
            raise ValueError('Source folder contains additional files; review these before consolidation')
    files = replacement_files(store,plan)
    remote = plan['release']['tracks']
    inspected = []
    for path in files:
        p = Path(path)
        if p.resolve()!=p or p.is_relative_to(folder): raise ValueError('Replacement overlaps the source or follows a symbolic link')
        row = read_metadata(p)
        if plan.get('kind')!='local' and str(row.get('tidal_album_id'))!=str(plan['release']['id']): continue
        inspected.append(dict(row,path=str(p),stamp=list(fingerprint(p))))
    if len(inspected)!=len(remote): raise ValueError('Download the complete replacement release first')
    used = set()
    for track in remote:
        hits = [t for t in inspected if (plan.get('kind')=='local' or str(t.get('tidal_track_id'))==str(track['id'])) and replacement_matches(t,track)]
        if len(hits)!=1 or hits[0]['path'] in used: raise ValueError('Replacement recording or duration could not be verified')
        used.add(hits[0]['path'])
    conflicts=dj_tag_conflicts(plan,inspected)
    if conflicts and not review_dj and plan.get('reviewed_dj_conflicts')!=conflicts:
        raise ValueError('Local BPM/key differs from the replacement. Review these differences before removing duplicates')
    # Header/tag agreement alone cannot establish that a completed replacement is playable.
    import soundfile as sf
    for number,track in enumerate(inspected,1):
        progress(f'Verifying replacement audio · {number}/{len(inspected)} · {Path(track["path"]).name}')
        if cancel():raise ValueError('Consolidation cancelled; original files retained')
        if decode:
            with sf.SoundFile(track['path']) as audio:
                frames=0
                for block in audio.blocks(blocksize=65536,dtype='int32'):
                    if cancel():raise ValueError('Consolidation cancelled; original files retained')
                    frames+=len(block)
                if not frames or frames!=audio.frames:raise ValueError('Replacement FLAC is incomplete or unreadable')
        if list(fingerprint(Path(track['path'])))!=track['stamp']:raise ValueError('Replacement changed during verification')
    return inspected


def consolidate(store, market, plan, trash=trash_file, cancel=lambda:False):
    """Called only after an explicit preview approval; replacement never removed."""
    inspected = validate_consolidation(store,plan,cancel=cancel)
    # Persist replacement links before retiring originals. If Trash fails they remain.
    from .linking import save_result
    roots = [Path(r['root']) for r in store.rows('SELECT root FROM roots')]
    for track in inspected:
        path = Path(track['path'])
        root = max((r for r in roots if path.is_relative_to(r)), key=lambda r:len(r.parts), default=path.parent)
        row = inspect_file(path,root)
        if track.get('tidal_album_id') and track.get('tidal_track_id'):
            row['catalogue_choice'] = dict(id=str(track['tidal_album_id']),track_id=track['tidal_track_id'],
                                           album=plan['release']['title'],artist=track['artist'],changes={},evidence='Verified consolidation')
            if not save_result(store,market,row,manual=True): raise ValueError('Replacement changed while saving links')
        stat=path.stat()
        with store.connect() as db:
            db.execute('INSERT OR IGNORE INTO roots VALUES(?,?,?)',(str(root),now(),'downloaded'))
            db.execute('INSERT OR REPLACE INTO local_files VALUES(?,?,?,?,?,?,1)',
                       (str(path),str(root),stat.st_size,stat.st_mtime_ns,json.dumps(read_metadata(path)),None))
    checked=validate_consolidation(store,plan,decode=False,cancel=cancel)
    if {t['path']:t['stamp'] for t in inspected}!={t['path']:t['stamp'] for t in checked}:raise ValueError('Replacement changed; refresh before consolidating')
    if cancel():raise ValueError('Consolidation cancelled; original files retained')
    trash(plan['folder'])
    if Path(plan['folder']).exists(): raise ValueError('Trash did not move the folder; originals retained')
    with store.connect() as db:
        for path in plan['stamps']:
            db.execute('UPDATE local_files SET present=0 WHERE path=?',(path,))
            db.execute('DELETE FROM track_links WHERE path=?',(path,))
    return len(plan['sources'])


def check_candidate_releases(store,market,root,api,cancel=lambda:False,progress=lambda s:None,limit=30):
    """Explicit, bounded refresh; reuse track lists with checked performer evidence."""
    local_tracks=[t for t in store.tracks() if Path(t['path']).is_relative_to(Path(root))]
    artists={t['artist'] for t in local_tracks}
    local_titles={norm(t.get('album','')) for t in local_tracks}
    ids={m['tidal_id'] for m in store.linked_mappings() if m['artist'] in artists}
    def captured(release):
        tracks=release.get('tracks') or []
        return bool(release.get('tracks_loaded') and tracks and
                    (release.get('optimization_credits_version')==1 or all(t.get('artists') or t.get('track_artist') for t in tracks)))
    detailed=set()
    for record in store.rows('SELECT payload FROM app_preferences WHERE key LIKE ?',(f'tag-review:{market}:%',)):
        release=json.loads(record['payload'])
        if captured(release):detailed.add(str(release['id']))
    candidates={};owners={}
    for cache in store.rows('SELECT artist_id,payload FROM catalogue WHERE market=?',(market,)):
        if cache['artist_id'] not in ids:continue
        owners[cache['artist_id']]=json.loads(cache['payload'])
        for release in owners[cache['artist_id']].get('releases',[]):
            if (not captured(release) and str(release['id']) not in detailed
                and release.get('available') is True and release_is_out(release)
                and int(release.get('track_count') or 0)>1):candidates[str(release['id'])]=release
    import time
    count=0
    for release in sorted(candidates.values(),key=lambda r:(norm(r.get('title','')) in local_titles,r.get('date','')),reverse=True)[:limit]:
        if cancel():break
        progress(f'Checking candidate release {count+1}/{min(len(candidates),limit)} · {release["title"]}')
        detail=dict(api.album_tag_details(release))
        # Album credits cannot establish who performed each recording. Fetch
        # track credits before deciding a source's audio can be retired.
        checked_tracks=[]
        for track in detail.get('tracks',[]):
            if cancel():return count
            checked_tracks.append(track if track.get('artists') or track.get('track_artist') else api.track_tag_details(track))
        detail['tracks']=checked_tracks
        detail['optimization_credits_checked']=True
        detail['optimization_credits_version']=1 if checked_tracks and all(t.get('artists') or t.get('track_artist') for t in checked_tracks) else 0
        store.save_preferences(f'tag-review:{market}:{release["id"]}',dict(detail,tag_checked_at=time.time(),metadata_schema=3))
        for artist_id,payload in owners.items():
            for i,old in enumerate(payload.get('releases',[])):
                if str(old['id'])==str(release['id']):
                    payload['releases'][i]=dict(detail,artist=old.get('artist') or detail.get('artist'))
                    store.save_catalogue(artist_id,market,payload)
        count+=1
    progress(f'{count} candidate track lists captured · run again to continue if more remain')
    return count


def optimization_cache_key(root,market,scope):
    return 'optimization-results:'+market+':'+scope+':'+str(root)

def optimization_signature(store,root,market,scope):
    import hashlib
    files=store.rows('SELECT path,size,mtime,metadata FROM local_files WHERE root=? AND present=1 ORDER BY path',(str(root),))
    remote=store.rows('SELECT artist_id,fetched FROM catalogue WHERE market=? ORDER BY artist_id',(market,)) if scope=='remote' else []
    return hashlib.sha256(json.dumps((files,remote),sort_keys=True).encode()).hexdigest()

def save_optimization_results(store,root,market,scope,rows):
    store.save_preferences(optimization_cache_key(root,market,scope),dict(version=1,
        signature=optimization_signature(store,root,market,scope),rows=rows))

def load_optimization_results(store,root,market,scope,allow_stale=False):
    saved=store.preferences(optimization_cache_key(root,market,scope))
    if saved.get('version')!=1:return None
    if not allow_stale and saved.get('signature')!=optimization_signature(store,root,market,scope):return None
    return saved.get('rows',[])
