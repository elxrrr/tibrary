"""Durable background associations; never writes music tags or moves files."""
import copy
import hashlib
import json
import time
from pathlib import Path
from .maintenance import fingerprint
from .tag_review import check_album_tags

FIELDS=('catalogue_choice','catalogue_options','catalogue_note','metadata_changes','dj_checks','equivalent_ids','recheck_required')
LINKER_VERSION=5


def needs_linker_upgrade(previous):
    if not previous or previous.get('linker_version',0)>=LINKER_VERSION:return False
    if previous.get('recheck_required'):return True
    if previous.get('status')!='review':return False
    note=(previous.get('catalogue_note') or previous.get('note') or '').casefold()
    return bool(previous.get('catalogue_options') or 'too many' in note or 'placement' in note)


def state_signature(store, market, root):
    """Fingerprint inputs which can change an automatic linking decision.

    This deliberately uses indexed metadata and durable remote/cache state.  It
    never opens audio files, so the timer can call it cheaply and safely.
    """
    local=store.rows('SELECT path,size,mtime,metadata,error,present FROM local_files WHERE root=? ORDER BY path',(str(root),))
    paths={r['path'] for r in local if r.get('present')}
    links=[r for r in store.rows('SELECT path,stamp,payload FROM track_links WHERE market=? ORDER BY path',(market,)) if r['path'] in paths]
    mappings=store.rows("SELECT artist,tidal_id,status,manual FROM mappings ORDER BY artist")
    extras=store.rows('SELECT artist,tidal_id FROM additional_mappings ORDER BY artist,tidal_id')
    catalogue=store.rows('SELECT artist_id,fetched FROM catalogue WHERE market=? ORDER BY artist_id',(market,))
    details=store.rows('SELECT key,length(payload) AS size,substr(payload,1,128) AS head,substr(payload,-128) AS tail FROM app_preferences WHERE key LIKE ? ORDER BY key',(f'tag-review:{market}:%',))
    payload=json.dumps((local,links,mappings,extras,catalogue,details),sort_keys=True,separators=(',',':'),default=str)
    return hashlib.sha256(payload.encode('utf-8')).hexdigest()


def pending_paths(store, market, rows, max_age_days=30):
    """Return only rows for which another automatic pass can do useful work."""
    saved=saved_links(store,market);now_at=time.time();pending=[]
    from .maintenance import first
    mappings={}
    for mapping in store.linked_mappings():mappings.setdefault(mapping['artist'],[]).append(str(mapping['tidal_id']))
    for row in rows:
        previous=matching_saved(row,saved)
        artist=first(row.get('tags',{}),'albumartist') or first(row.get('tags',{}),'artist') or ''
        current_ids=row.get('link_artist_ids') or mappings.get(artist,[])
        mapping_changed=bool(previous and previous.get('status')!='linked' and previous.get('artist_ids',[])!=current_ids)
        fresh=bool(previous and now_at-previous.get('checked_at',0)<max_age_days*86400)
        if not previous or mapping_changed or needs_linker_upgrade(previous):
            pending.append(row);continue
        if previous.get('status')=='linked' and not previous.get('dj_complete') and not fresh:
            pending.append(row);continue
        # A saved choice/unmatched result is intentionally stable until it
        # expires or the local/remote state changes.  It must not create a loop.
        if previous.get('status') in ('review','unmatched') and not fresh:
            pending.append(row)
    return pending


def saved_links(store, market):
    return {r['path']:(json.loads(r['stamp']),json.loads(r['payload']))
            for r in store.rows('SELECT path,stamp,payload FROM track_links WHERE market=?',(market,))}


def stamps_match(stamp_a, stamp_b):
    if not stamp_a or not stamp_b: return False
    t_a, t_b = tuple(stamp_a), tuple(stamp_b)
    if t_a == t_b: return True
    if len(t_a) >= 4 and len(t_b) == 2: return (t_a[2], t_a[3]) == t_b
    if len(t_a) == 2 and len(t_b) >= 4: return t_a == (t_b[2], t_b[3])
    return False


def compatible_tags(row, result):
    before=result.get('local_tags')
    if before is None: return False
    dur_row = float(row.get('duration') or 0)
    dur_res = float(result.get('duration') or 0)
    if dur_row > 0 and dur_res > 0 and abs(dur_row - dur_res) > .02: return False
    expected={**before,**result.get('metadata_changes',{}),**result.get('catalogue_choice',{}).get('changes',{})}
    from .maintenance import normalized_date,disc_changes
    after=row.get('tags',{})
    identity={'albumartist','artist','album','title','isrc','tidal_track_id','tidal_album_id','tracknumber','discnumber','date','disctotal','totaldiscs','tracktotal','totaltracks'}
    for key in identity:
        old=before.get(key,[]);new=after.get(key,[])
        if old==new or new==expected.get(key,[]):continue
        if key in ('tracknumber','discnumber','tracktotal','totaltracks','disctotal','totaldiscs'):
            def numeric(values):
                try:return [tuple(int(part.strip()) for part in str(v).split('/')) for v in values]
                except ValueError:return values
            if numeric(old)==numeric(new):continue
        if key=='date' and [normalized_date(v) for v in old]==[normalized_date(v) for v in new]:continue
        if key in ('discnumber','disctotal','totaldiscs') and new==dict(before,**disc_changes(before)[0]).get(key,[]):continue
        return False
    return True


def matching_saved(row, saved):
    previous=saved.get(row['path'])
    if not previous:return None
    return previous[1] if stamps_match(previous[0], row.get('stamp', ())) or compatible_tags(row,previous[1]) else None


def rebased_payload(row, payload):
    from .enrichment import tag_aliases
    result=copy.deepcopy(payload);tags=row.get('tags',{})
    for option in result.get('catalogue_options',[]):
        option['changes']={k:v for k,v in option['changes'].items() if tags.get(k)!=v}
    choice=result.get('catalogue_choice')
    if choice:choice['changes']={k:v for k,v in choice['changes'].items() if tags.get(k)!=v}
    result['metadata_changes']={k:v for k,v in result.get('metadata_changes',{}).items() if not any(tags.get(a) for a in (('initialkey',) if k=='initialkey' else tag_aliases(k)))}
    from .musical_keys import camelot_key
    if result['metadata_changes'].get('initialkey'):
        converted=[camelot_key(v) for v in result['metadata_changes']['initialkey']]
        if all(converted):result['metadata_changes']['initialkey']=converted
        else:result['metadata_changes'].pop('initialkey')
    result.update(local_tags=copy.deepcopy(tags),duration=row.get('duration',0))
    return result


def attach_links(rows, store, market):
    saved=saved_links(store,market)
    mqa_files=store.preferences('mqa-audit').get('files',{})
    for row in rows:
        audited=mqa_files.get(row['path'],{})
        if audited and stamps_match(audited.get('stamp'),row.get('stamp',())):
            row['mqa_audit']=copy.deepcopy(audited.get('result',{}))
        else:row.pop('mqa_audit',None)
        result=matching_saved(row,saved)
        if not result:
            if row['path'] in saved:
                for field in (*FIELDS,'linked_ids','link_note'):row.pop(field,None)
            continue
        result=rebased_payload(row,result)
        if not stamps_match(saved[row['path']][0], row.get('stamp', ())) or 'local_tags' not in saved[row['path']][1]:
            persist_rebased(store,market,row,result)
        for field in FIELDS:
            if field in result:row[field]=copy.deepcopy(result[field])
        row['linked_ids']=result.get('ids',{})
        row['equivalent_ids']=copy.deepcopy(result.get('equivalent_ids',[]))
        row['link_note']=result['note']
        row['metadata_note']=result.get('catalogue_note',result['note'])
    hydrate_match_labels(rows,store,market)
    return rows


def persist_rebased(store,market,row,result):
    try:
        fp = fingerprint(Path(row['path']))
        if not stamps_match(fp, row.get('stamp', ())): return
    except OSError:return
    stamp_to_save = fp if len(row.get('stamp', ())) < 5 else row['stamp']
    with store.connect() as db:
        db.execute('INSERT OR REPLACE INTO track_links VALUES(?,?,?,?)',(row['path'],market,json.dumps(stamp_to_save),json.dumps(result)))


def retain_after_apply(store,market,old,new):
    records=store.rows('SELECT stamp,payload FROM track_links WHERE path=? AND market=?',(old['path'],market))
    if not records or not stamps_match(json.loads(records[0]['stamp']), old.get('stamp', ())):return
    result=json.loads(records[0]['payload'])
    result.setdefault('local_tags',old.get('tags',{}));result.setdefault('duration',old.get('duration',0))
    if compatible_tags(new,result):persist_rebased(store,market,new,rebased_payload(new,result))
    if old['path']!=new['path']:
        with store.connect() as db:db.execute('DELETE FROM track_links WHERE path=? AND market=?',(old['path'],market))


def invalidate_related_links(store,market,old,new):
    """Schedule only the affected local release group for another link pass."""
    from .release_matching import group_key
    affected={group_key(old),group_key(new)};updates=[]
    for record in store.rows('SELECT path,payload FROM track_links WHERE market=?',(market,)):
        try:payload=json.loads(record['payload'])
        except (TypeError,ValueError):continue
        if payload.get('manual'):continue
        candidate=dict(path=record['path'],tags=payload.get('local_tags',{}),duration=payload.get('duration',0))
        if group_key(candidate) not in affected:continue
        payload.update(recheck_required=True,checked_at=0,linker_version=0)
        updates.append((json.dumps(payload),record['path'],market))
    if updates:
        with store.connect() as db:db.executemany('UPDATE track_links SET payload=? WHERE path=? AND market=?',updates)
    return len(updates)


def save_result(store,market,row,manual=False):
    # Local cleanup may run while the API is working. Never associate its old
    # response with a file that has since changed, moved or disappeared.
    try:
        fp = fingerprint(Path(row['path']))
        if not stamps_match(fp, row.get('stamp', ())): return False
    except OSError:return False
    options=row.get('catalogue_options',[])
    chosen=row.get('catalogue_choice')
    candidates=[chosen] if chosen else options
    identities={(str(o['id']),str(o.get('track_id',''))) for o in candidates if o and (chosen or ((o.get('recording_verified') or o.get('whole_release_verified')) and o.get('structure', {}).get('compatible', True)))}
    ids={}
    if len(identities)==1 and all(track for _,track in identities):
        album,track=next(iter(identities));ids=dict(album_id=album,track_id=track)
    equivalent_ids=[]
    if ids:
        source=chosen or next((o for o in options if str(o.get('id'))==ids['album_id'] and str(o.get('track_id'))==ids['track_id']),None)
        equivalent_ids=copy.deepcopy((source or {}).get('equivalent_placements',[]))
        if not equivalent_ids:equivalent_ids=[dict(album_id=ids['album_id'],track_id=ids['track_id'])]
    checks=[c for c in row.get('dj_checks',[]) if c.get('track_id')==ids.get('track_id') and c.get('album_id')==ids.get('album_id')]
    dj=checks[0] if checks else {}
    status='linked' if ids else ('review' if options else 'unmatched')
    note={'linked':'Recording and release linked','review':'Needs review · more than one placement or no recording identifier',
          'unmatched':'No verified match · review tags or artist links'}[status]
    if ids and dj:
        if dj.get('status')=='incomplete':
            note+=' · API check incomplete · resume to retry'
        else:
            note+=f' · BPM: {", ".join(dj.get("bpm",[])) or "not supplied"} · Key: {", ".join(dj.get("key",[])) or "not supplied"}'
    from .maintenance import first
    artist_ids = row.get('link_artist_ids')
    if artist_ids is None:
        artist_name = first(row.get('tags', {}), 'albumartist') or first(row.get('tags', {}), 'artist') or ''
        artist_ids = [str(m['tidal_id']) for m in store.linked_mappings() if m['artist'] == artist_name]
    payload={k:copy.deepcopy(row[k]) for k in FIELDS if k in row}
    payload.update(local_tags=copy.deepcopy(row.get('tags',{})),duration=row.get('duration',0),manual=manual,artist_ids=artist_ids,ids=ids,
                   equivalent_ids=equivalent_ids,status=status,note=note,checked_at=time.time(),dj_complete=dj.get('status')=='checked',
                   recheck_required=False,linker_version=LINKER_VERSION)
    stamp_to_save = fp if len(row.get('stamp', ())) < 5 else row['stamp']
    with store.connect() as db:
        existing=db.execute('SELECT stamp,payload FROM track_links WHERE path=? AND market=?',(row['path'],market)).fetchone()
        if existing and not manual and stamps_match(json.loads(existing[0]), stamp_to_save):
            previous=json.loads(existing[1])
            if previous.get('manual'):
                if previous.get('ids')!=ids:return False
                payload['manual']=True
                if previous.get('catalogue_choice'):payload['catalogue_choice']=previous['catalogue_choice']
            elif previous.get('ids') and not ids:
                # A timeout, duplicate edition or newly ambiguous catalogue
                # response is not evidence that a durable recording link was
                # wrong. Keep it until a replacement is verified, the local
                # file changes, or the user explicitly unlinks it.
                recheck_note = row.get('catalogue_note') or note
                payload=copy.deepcopy(previous)
                payload.update(status='linked', checked_at=time.time(),
                               note='Recording and release link retained · latest recheck was inconclusive',
                               last_recheck_note=recheck_note,recheck_required=False,linker_version=LINKER_VERSION)
                ids=copy.deepcopy(payload['ids'])
        db.execute('INSERT OR REPLACE INTO track_links VALUES(?,?,?,?)',(row['path'],market,json.dumps(stamp_to_save),json.dumps(payload)))
    row['linked_ids']=copy.deepcopy(ids)
    row['equivalent_ids']=copy.deepcopy(payload.get('equivalent_ids',[]))
    row['recheck_required']=False
    return True


def link_recordings(rows,store,market,api,dj_lookup=None,cancel=lambda:False,progress=lambda _:None,recheck=False,context_rows=None,on_result=lambda row:None):
    saved=saved_links(store,market)
    from .maintenance import first
    mappings={}
    for mapping in store.linked_mappings():mappings.setdefault(mapping['artist'],[]).append(str(mapping['tidal_id']))
    unlinked=[]
    need_retry=[]
    for row in rows:
        previous=matching_saved(row,saved)
        artist = first(row.get('tags', {}), 'albumartist') or first(row.get('tags', {}), 'artist') or ''
        current_artist_ids = row.get('link_artist_ids') or mappings.get(artist, [])
        mapping_changed = bool(previous and previous.get('status') != 'linked' and previous.get('artist_ids', []) != current_artist_ids)
        is_fresh = bool(previous and (time.time() - previous.get('checked_at', 0) < 86400 * 30))
        already_has_placements = bool(previous and previous.get('status') == 'review' and previous.get('catalogue_options'))
        already_unmatched = bool(previous and previous.get('status') == 'unmatched' and is_fresh)

        if recheck or mapping_changed or not previous or needs_linker_upgrade(previous):
            unlinked.append(row)
        elif not (already_has_placements or already_unmatched):
            if not previous.get('dj_complete'):
                need_retry.append(row)
    pending = unlinked if unlinked else need_retry
    progress(f'Recording links · {len(rows)-len(pending):,} saved results reused · {len(pending):,} to check')
    completed=0
    processed=0
    def record(row):
        nonlocal completed, processed
        processed += 1
        tags = row.get('tags', {})
        title = first(tags, 'title') or Path(row['path']).stem
        artist = first(tags, 'artist') or first(tags, 'albumartist')
        desc = f"{artist} — {title}" if artist else title
        options = row.get('catalogue_options', [])
        chosen = row.get('catalogue_choice')
        candidates = [chosen] if chosen else options
        identities = {(str(o['id']), str(o.get('track_id', ''))) for o in candidates if o and (chosen or ((o.get('recording_verified') or o.get('whole_release_verified')) and o.get('structure', {}).get('compatible', True)))}
        is_linked = (len(identities) == 1 and all(track for _, track in identities))
        if not cancel() and save_result(store, market, row):
            persisted_ids=copy.deepcopy(row.get('linked_ids',{}))
            is_linked=bool(persisted_ids.get('album_id') and persisted_ids.get('track_id'))
            updated=copy.deepcopy(row)
            updated['linked_ids']=persisted_ids if is_linked else {}
            on_result(updated)
            if is_linked:
                completed += 1
                progress(f'Linked ({processed:,}/{len(pending):,}) · {desc}')
            else:
                progress(f'Needs review ({processed:,}/{len(pending):,}) · {desc}')
        elif not cancel():
            progress(f'Needs review ({processed:,}/{len(pending):,}) · {desc}')
    if pending:
        # Routine catalogue chatter belongs to the API, not the track-level activity list.
        def linking_progress(message):
            if message.startswith(('Searching for:', 'Linked (', 'Needs review (')) or any(
                word in message.casefold() for word in ('timeout', 'timed out', 'retry', 'rate limit', '429', 'failed', 'error', 'cancel')
            ):
                progress(message)
        check_album_tags(pending,store,market,api,cancel,linking_progress,enrich=True,correct_metadata=False,
                         dj_lookup=dj_lookup,on_result=record,force=False,cache_first=True,context_rows=context_rows if context_rows is not None else rows)
    return f'Linking {"paused" if cancel() else "finished"} · {completed:,} results saved · resume reuses completed work'


def online_match_label(row):
    """Use the same release label for saved links and newly accepted proposals."""
    ids=row.get('linked_ids') or {}
    choice=row.get('catalogue_choice') or {}
    album_id=str(ids.get('album_id') or choice.get('id') or '')
    options=row.get('catalogue_options') or []
    fallback=choice if str(choice.get('id'))==album_id else row.get('linked_release',{})
    match=next((o for o in options if str(o.get('id'))==album_id), fallback)
    artist=match.get('artist') or 'Artist unavailable'
    album=match.get('album') or 'Album unavailable'
    return f'{artist} — {album} [{album_id}]'


def hydrate_match_labels(rows,store,market):
    """Hydrate names and edition counts in batches, including older saved links."""
    missing={str(r['linked_ids']['album_id']) for r in rows if r.get('linked_ids',{}).get('album_id')
             and not r.get('catalogue_choice') and not any(str(o.get('id'))==str(r['linked_ids']['album_id']) for o in r.get('catalogue_options',[]))}
    for row in rows:
        for option in [*row.get('catalogue_options',[]),row.get('catalogue_choice') or {}]:
            if option.get('id') and not option.get('release_counts'):missing.add(str(option['id']))
    details={};identifiers=sorted(missing)
    for start in range(0,len(identifiers),400):
        keys=[f'tag-review:{market}:{id}' for id in identifiers[start:start+400]]
        for record in store.rows('SELECT payload FROM app_preferences WHERE key IN ('+','.join('?' for _ in keys)+')',keys):
            release=json.loads(record['payload']);details[str(release['id'])]=release
    for row in rows:
        ident=str(row.get('linked_ids',{}).get('album_id',''))
        if ident in details:
            release=details[ident]
            row['linked_release']=dict(album=release.get('title'),artist=release.get('artist') or ', '.join(release.get('album_artists',[])))
        for option in [*row.get('catalogue_options',[]),row.get('catalogue_choice') or {}]:
            release=details.get(str(option.get('id')), {})
            if option.get('release_counts') or not release.get('tracks_loaded'):continue
            tracks=release.get('tracks',[])
            track=next((t for t in tracks if str(t.get('id'))==str(option.get('track_id'))),None)
            if not track:continue
            disc=int(track.get('disc_number') or 1)
            alignment=option.get('structure',{}).get('alignments',{}).get(row['path'],{})
            if (alignment.get('disc_number'),alignment.get('track_number'))!=(disc,track.get('track_number')):continue
            option['release_counts']=dict(tracktotal=sum(int(t.get('disc_number') or 1)==disc for t in tracks),
                disctotal=release.get('disc_count') or max((int(t.get('disc_number') or 1) for t in tracks),default=1))
