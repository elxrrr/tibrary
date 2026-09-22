"""Previewed FLAC maintenance. No automatic deletion or backup; catalogue repairs require a reviewed plan."""
import copy
import errno
import hashlib
import json
import os
import re
import shutil
import tempfile
import unicodedata
from collections import Counter
from datetime import date
from pathlib import Path
import subprocess
import sys
from .tag_io import FLAC
from .core import now, read_metadata


def fingerprint(path):
    s = path.stat()
    return (s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns, s.st_ctime_ns)


def normalized_date(value):
    value = re.sub(r'[T\s]\d{2}:\d{2}:\d{2}.*$', '', value.strip())
    if re.fullmatch(r'\d{4}/\d{2}/\d{2}', value): value = value.replace('/', '-')
    try:
        if re.fullmatch(r'\d{4}', value): date(int(value), 1, 1)
        elif re.fullmatch(r'\d{4}-\d{2}', value): date.fromisoformat(value + '-01')
        elif re.fullmatch(r'\d{4}-\d{2}-\d{2}',value): date.fromisoformat(value)
        else:return None
        return value
    except ValueError:
        return None


def first(tags, key):
    if not tags: return ''
    if key in ('albumartist', 'artist'):
        val = tags.get(key)
        if not val and key == 'artist': val = tags.get('albumartist')
        if isinstance(val, (list, tuple)):
            return ', '.join(dict.fromkeys(str(v).strip() for v in val if str(v).strip()))
        return str(val).strip() if val else ''
    val = tags.get(key)
    if not val and key == 'tracknumber': val = tags.get('track')
    if not val and key == 'discnumber': val = tags.get('disc')
    if not val and key == 'album': val = tags.get('albumtitle')
    if isinstance(val, (list, tuple)):
        return str(val[0]).strip() if val else ''
    return str(val).strip() if val else ''


def date_changes(tags):
    edits = {}
    for key in ('date', 'originaldate', 'releasedate'):
        values = tags.get(key, [])
        if not values: continue
        cleaned = [normalized_date(v) for v in values]
        if any(v is None for v in cleaned):
            return {}, 'Invalid or ambiguous ' + key
        if values != cleaned:
            edits[key] = cleaned
    return edits, ''


def disc_changes(tags):
    """Use two-digit padded integers (matching TIDAL) and a separate total, preserving total aliases."""
    keys=('discnumber','disctotal','totaldiscs')
    if not any(tags.get(k) for k in keys): return {}, ''
    if any(len(tags.get(k,[]))>1 for k in keys):return {}, 'Multiple disc values; choose one before normalising'
    raw=first(tags,'discnumber')
    if raw and not re.fullmatch(r'0*[1-9]\d*(?:\s*/\s*0*[1-9]\d*)?',raw):
        return {}, 'Invalid disc number; unchanged'
    parts=raw.split('/') if raw else []
    totals=[first(tags,k) for k in ('disctotal','totaldiscs') if tags.get(k)]
    if len(parts)>1:totals.append(parts[1].strip())
    if any(not v.isdigit() or int(v)<1 for v in totals):return {}, 'Invalid disc total; unchanged'
    if len({int(v) for v in totals})>1:return {}, 'Conflicting disc totals; unchanged'
    number=int(parts[0]) if parts else None
    total=int(totals[0]) if totals else None
    if number and total and number>total:return {}, 'Disc number exceeds total; unchanged'
    proposed={}
    if number:proposed['discnumber']=[f'{number:02d}']
    if total:
        proposed['disctotal']=[f'{total:02d}']
        if 'totaldiscs' in tags:proposed['totaldiscs']=[f'{total:02d}']
    return {key:value for key,value in proposed.items() if tags.get(key)!=value}, ''


def track_changes(tags):
    """Reuse the same validated numeric policy for track number/total tags."""
    translated={k.replace('track','disc'):v for k,v in tags.items() if k in ('tracknumber','tracktotal','totaltracks')}
    edits,issue=disc_changes(translated)
    return {k.replace('disc','track'):v for k,v in edits.items()},issue.replace('disc','track').replace('Disc','Track')


from .organisation import safe_component, name_key, layout_path, DEFAULT_LAYOUT


def destination(root, tags, disc_folder=False, current_path=None, layout=None):
    _,disc_issue=disc_changes(tags)
    if disc_issue:raise ValueError(disc_issue+'; resolve before moving')
    disc=first(tags,'discnumber').split('/')
    total=first(tags,'disctotal') or first(tags,'totaldiscs') or (disc[1] if len(disc)>1 else '')
    relevant=bool(disc[0].isdigit() and (disc_folder or int(disc[0])>1 or (total.isdigit() and int(total)>1)))
    return layout_path(root,tags,(normalized_date(first(tags,'date')) or '')[:4],relevant,current_path,
                       (layout or {}).get('template',DEFAULT_LAYOUT))


def inspect_file(path, root, layout=None):
    path,root=Path(path),Path(root)
    row=dict(path=str(path),root=str(root),target=str(path),changes={},issues=[],blocked='',organise=False,layout=layout or {})
    try:
        before=fingerprint(path);audio=FLAC(path)
        front=next((p for p in audio.pictures if p.type==3),None)
        from .artwork import cover_dimensions
        row['cover_size']=cover_dimensions(front.data) if front else None
        row.update(stamp=before,tags={k:list(v) for k,v in audio.tags.items()},has_artwork=bool(audio.pictures),duration=audio.info.length)
        if fingerprint(path)!=before:raise ValueError('File changed during inspection; inspect again')
    except Exception as exc:row['blocked']=str(exc)
    return row


def plan_library(root, repair=True, dates=True, organise=False, cancel=lambda:False, progress=lambda s:None, discs=True, layout=None):
    root=Path(root).expanduser().resolve(strict=True)
    if not root.is_dir():raise ValueError('Choose a library folder')
    snapshot=[]
    from .file_services import inventory
    entries, complete = inventory(root, {'.flac'}, cancel, progress)
    for name, _, _ in entries:
        if cancel():break
        snapshot.append(inspect_file(Path(name),root,layout))
        if len(snapshot)%100==0:progress(f'Inspected {len(snapshot):,} FLAC files')
    progress(f'{"Partial inspection" if cancel() else "Inspection complete"} · {len(snapshot):,} FLAC files · no files changed')
    return replan(snapshot,repair,dates,organise,discs)


def stray_changes(snapshot):
    """Detect single/stray tracks whose album title and metadata match an established multi-track album in the library."""
    from collections import defaultdict
    from .core import norm
    import re
    albums = defaultdict(list)
    for row in snapshot:
        tags = row.get('tags', {})
        alb = first(tags, 'album').strip()
        if not alb or norm(alb) in {'unknown', 'unknown release', 'singles', 'single'}:
            continue
        albums[norm(alb)].append(row)
    stray_edits = {}
    for norm_alb, rows in albums.items():
        if len(rows) < 2:
            continue
        by_folder = defaultdict(list)
        for r in rows:
            p = Path(r['path']).parent
            if re.fullmatch(r'disc\s*0*[1-9]\d*', p.name, re.I):
                p = p.parent
            by_folder[str(p)].append(r)
        
        by_artist = defaultdict(list)
        for r in rows:
            tags = r.get('tags', {})
            art = first(tags, 'albumartist') or first(tags, 'artist')
            by_artist[art].append(r)

        if len(by_folder) >= 2:
            sorted_folders = sorted(by_folder.items(), key=lambda x: len(x[1]), reverse=True)
            main_folder, main_tracks = sorted_folders[0]
            candidate_strays = [sr for _, s_tracks in sorted_folders[1:] if len(s_tracks) <= 2 for sr in s_tracks]
        elif len(by_artist) >= 2:
            sorted_artists = sorted(by_artist.items(), key=lambda x: len(x[1]), reverse=True)
            _, main_tracks = sorted_artists[0]
            candidate_strays = [sr for _, s_tracks in sorted_artists[1:] if len(s_tracks) <= 2 for sr in s_tracks]
        else:
            continue

        if len(main_tracks) < 2:
            continue

        main_artist = first(main_tracks[0].get('tags', {}), 'albumartist') or first(main_tracks[0].get('tags', {}), 'artist')
        if not main_artist:
            continue
        main_art_norm = norm(main_artist)
        main_ids = {first(r.get('tags', {}), 'tidal_album_id') for r in main_tracks} - {''}
        main_total = first(main_tracks[0].get('tags', {}), 'tracktotal') or first(main_tracks[0].get('tags', {}), 'totaltracks')
        main_tracknums = {first(r.get('tags', {}), 'tracknumber').split('/')[0] for r in main_tracks} - {''}
        main_year = first(main_tracks[0].get('tags', {}), 'date')[:4]

        for sr in candidate_strays:
            sr_tags = sr.get('tags', {})
            sr_art = first(sr_tags, 'artist')
            sr_albumart = first(sr_tags, 'albumartist')

            if sr_albumart and norm(sr_albumart) == main_art_norm:
                continue

            sr_id = first(sr_tags, 'tidal_album_id')
            sr_total = first(sr_tags, 'tracktotal') or first(sr_tags, 'totaltracks')
            sr_num = first(sr_tags, 'tracknumber').split('/')[0]
            sr_year = first(sr_tags, 'date')[:4]

            id_match = bool(sr_id and sr_id in main_ids)
            artist_overlap = bool(main_art_norm in norm(sr_art) or norm(sr_art) in main_art_norm or (sr_albumart and main_art_norm in norm(sr_albumart)))
            track_complement = bool(sr_total and sr_total == main_total and sr_num and sr_num not in main_tracknums and (not sr_year or sr_year == main_year))

            if id_match or artist_overlap or track_complement:
                stray_edits[sr['path']] = ({'albumartist': [main_artist]}, f'Reunite stray track with album artist “{main_artist}”')
    return stray_edits


def detect_superseded_singles(snapshot):
    """
    Detect standalone single tracks that have since been superseded by a wider multi-track EP/Album
    release containing the exact same recording (matching title and duration/isrc).
    Handles both:
    1. Separate folders (single folder with 1 track vs album folder with >= 2 tracks).
    2. Same folder (single file placed into album folder creating duplicate title/conflicting track 1).
    """
    import re
    from collections import defaultdict, Counter
    from .core import norm, title_key
    from .release_matching import replacement_matches, position

    def track_number_of(r):
        tags = r.get('tags', {})
        val = first(tags, 'tracknumber') or first(tags, 'track')
        if not val or not str(val).split('/')[0].strip().isdigit():
            m = re.match(r'^(\d+)', Path(r['path']).stem)
            if m: val = str(int(m.group(1)))
        return str(val).split('/')[0].strip() if val else ''

    folder_tracks = defaultdict(list)
    for row in snapshot:
        if row.get('blocked') or 'tags' not in row:
            continue
        p = row.get('path')
        if p:
            folder_tracks[str(Path(p).parent)].append(row)

    superseded = {}

    # Index album tracks (folders with >= 2 tracks) by (norm_artist, title_key)
    album_tracks_by_song = defaultdict(list)
    for folder, tr_list in folder_tracks.items():
        if len(tr_list) >= 2:
            for r in tr_list:
                tags = r.get('tags', {})
                art = first(tags, 'albumartist') or first(tags, 'artist')
                title = first(tags, 'title')
                if art and title:
                    album_tracks_by_song[(norm(art), title_key(title))].append(r)

    # Case 1: Separate folders (t_a is in a single-track folder, t_b is in a multi-track folder)
    for folder, tr_list in folder_tracks.items():
        if len(tr_list) == 1:
            t_a = tr_list[0]
            tags_a = t_a.get('tags', {})
            art_a = first(tags_a, 'albumartist') or first(tags_a, 'artist')
            title_a = first(tags_a, 'title')
            if not art_a or not title_a:
                continue
            candidates = album_tracks_by_song.get((norm(art_a), title_key(title_a)), [])
            dur_a = float(t_a.get('duration') or 0)
            isrc_a = first(tags_a, 'isrc').strip().upper()
            num_a = track_number_of(t_a)

            for t_b in candidates:
                if str(Path(t_b['path']).parent) == folder:
                    continue
                tags_b = t_b.get('tags', {})
                dur_b = float(t_b.get('duration') or 0)
                isrc_b = first(tags_b, 'isrc').strip().upper()
                same_isrc = bool(isrc_a and isrc_b and isrc_a == isrc_b)
                same_dur = bool(dur_a and dur_b and abs(dur_a - dur_b) <= 3.0)
                if replacement_matches(t_a,t_b):
                    folder_b = Path(t_b['path']).parent
                    alb_b = first(tags_b, 'album')
                    num_b = track_number_of(t_b)
                    rel_b_count = len(folder_tracks[str(folder_b)])
                    superseded[t_a['path']] = {
                        'single_title': title_a,
                        'single_track': num_a or '1',
                        'album_title': alb_b or folder_b.name,
                        'album_track': num_b or '—',
                        'album_path': t_b['path'],
                        'album_stamp': t_b.get('stamp'),
                        'reason': f"Superseded single · matching recording retained: '{alb_b or folder_b.name}' · disc {position(t_b)[0]}, track {num_b or '—'} · {first(tags_b,'artist')} · ISRC {isrc_b} · {dur_b:.1f}s"
                    }
                    break

    # Case 2: Same folder (e.g. single downloaded into album folder)
    for folder, tr_list in folder_tracks.items():
        if len(tr_list) >= 2:
            isrc_map = defaultdict(list)
            title_map = defaultdict(list)
            dates = [first(r.get('tags', {}), 'date')[:10] for r in tr_list if first(r.get('tags', {}), 'date')]
            common_date = Counter(dates).most_common(1)[0][0] if dates else ''

            for r in tr_list:
                isrc = first(r.get('tags', {}), 'isrc').strip().upper()
                if isrc: isrc_map[isrc].append(r)
                title = first(r.get('tags', {}), 'title')
                if title: title_map[title_key(title)].append(r)

            # Check matching ISRC in same folder
            for isrc, files in isrc_map.items():
                if len(files) >= 2:
                    album_track = next((o for o in files if track_number_of(o) not in ('1', '01', '')), None)
                    if not album_track and common_date:
                        album_track = next((o for o in files if first(o.get('tags', {}), 'date')[:10] == common_date), None)
                    if album_track:
                        tnum_o = track_number_of(album_track)
                        date_o = first(album_track.get('tags', {}), 'date')[:10]
                        alb_name = first(album_track.get('tags', {}), 'album') or Path(folder).name
                        for f in files:
                            if f['path'] == album_track['path'] or f['path'] in superseded: continue
                            if not replacement_matches(f,album_track):continue
                            tnum_f = track_number_of(f)
                            date_f = first(f.get('tags', {}), 'date')[:10]
                            is_f_single = (tnum_f in ('1', '01', '') and tnum_o not in ('1', '01', ''))
                            is_f_older = bool(date_f and date_o and date_f < date_o and date_o == common_date)
                            if is_f_single or is_f_older:
                                title_f = first(f.get('tags', {}), 'title') or Path(f['path']).stem
                                superseded[f['path']] = {
                                    'single_title': title_f,
                                    'single_track': tnum_f or '1',
                                    'album_title': alb_name,
                                    'album_track': tnum_o or '—',
                                    'album_path': album_track['path'],
                                    'album_stamp': album_track.get('stamp'),
                                    'reason': f"Superseded single · matching recording retained: '{alb_name}' · disc {position(album_track)[0]}, track {tnum_o or '—'} · {first(album_track.get('tags',{}),'artist')} · ISRC {isrc}"
                                }

    return superseded


def trash_file(path_str):
    """Move to system Trash; never permanently delete on failure."""
    p = Path(path_str)
    if not p.exists():
        return
    try:
        import send2trash
        send2trash.send2trash(str(p))
        return
    except Exception:
        pass
    if sys.platform == 'darwin':
        try:
            esc = str(p.resolve()).replace('\\', '\\\\').replace('"', '\\"')
            cmd = f'tell application "Finder" to delete POSIX file "{esc}"'
            res = subprocess.run(['osascript', '-e', cmd], capture_output=True, text=True, timeout=20)
            if res.returncode == 0:
                return
        except Exception:
            pass
    raise ValueError('Could not move the file to Trash. The original file has been retained.')


def replan(snapshot, repair=True, dates=True, organise=False, discs=True, duplicate_tracks=False):
    """Build another preview from inspected tags. No file contents are reread."""
    plans=copy.deepcopy(snapshot)
    from .number_repairs import number_repairs
    numeric=number_repairs(snapshot) if discs else {}
    for row in plans:
        root,path=Path(row['root']),Path(row['path'])
        row.update(target=row['path'],layout_target=row['path'],changes={},issues=[],organise=organise,multi_disc=False)
        row.pop('result',None)
        if row.get('artwork_change'):row['artwork_proposal']=row['artwork_change']
        row.pop('artwork_change',None)
        if row.get('covers_enabled',True) and row.get('artwork_proposal'):row['artwork_change']=row['artwork_proposal']
        if row['blocked'] or 'tags' not in row:continue
        tags=row['tags']
        row['disc_folder']=bool(re.fullmatch(r'disc\s+0*[1-9]\d*',path.parent.name,re.I))
        disc_edits,disc_issue=disc_changes(tags)
        track_edits,track_issue=track_changes(tags)
        if discs:
            edits,notes=numeric.get(row['path'],({},[]))
            row['changes'].update(edits);row['issues'].extend(notes)
            effective=dict(tags,**edits)
            _,disc_issue=disc_changes(effective);_,track_issue=track_changes(effective)
        if track_issue:row['issues'].append(track_issue)
        if disc_issue:row['issues'].append(disc_issue)
        albumartist=first(tags,'albumartist')
        for key in ('albumartist','artist','album','title','tracknumber','date'):
            if not first(tags,key):row['issues'].append('Missing '+key)
        if not (first(tags,'bpm') or first(tags,'tempo')):row['issues'].append('Missing BPM')
        if not (first(tags,'key') or first(tags,'initialkey') or first(tags,'tkey')):row['issues'].append('Missing musical key')
        if not row.get('has_artwork'): row['issues'].append('No embedded artwork')
        elif row.get('cover_size')!=(1280,1280):row['issues'].append('Front cover is not 1280 × 1280')
        if not first(tags,'isrc'): row['issues'].append('No ISRC (optional)')
        # Folder discrepancies are informational, never evidence for retagging.
        parts=path.relative_to(root).parts
        folder_artist=parts[0] if len(parts)>=3 and row.get('layout',{}).get('template',DEFAULT_LAYOUT).startswith('{albumartist}/') else ''
        try:artist_component=safe_component(albumartist) if albumartist else ''
        except ValueError:artist_component=''
        if not albumartist:
            row['issues'].append('Verify album artist with TIDAL or set it explicitly; folders are not tag evidence')
        elif folder_artist and name_key(folder_artist).casefold() != name_key(artist_component).casefold():
            row['issues'].append('Artist folder differs from album artist')
        for key in ('date','originaldate','releasedate'):
            values=tags.get(key,[])
            cleaned=[normalized_date(v) for v in values]
            if any(v is None for v in cleaned):row['issues'].append('Invalid or ambiguous '+key)
            elif dates and values!=cleaned:row['changes'][key]=cleaned
        if row.get('enrich_enabled'):
            row['changes'].update({k:v for k,v in row.get('metadata_changes',{}).items() if not first(tags,k)})
        if row.get('catalogue_note'): row['issues'].append(row['catalogue_note'])
        if repair and row.get('catalogue_choice'):
            row['changes'].update(row['catalogue_choice']['changes'])
    albums={}
    for row in plans:
        if 'tags' in row and not row['blocked']:
            effective=dict(row['tags'],**row['changes'])
            source_folder=Path(row['path']).parent
            if row.get('disc_folder'):source_folder=source_folder.parent
            albums.setdefault((str(source_folder),first(effective,'albumartist'),first(effective,'album')),[]).append(row)
    for rows in albums.values():
        multi=any(first(dict(r['tags'],**r['changes']),'discnumber').split('/')[0].isdigit() and int(first(dict(r['tags'],**r['changes']),'discnumber').split('/')[0])>1 for r in rows)
        if multi:
            for row in rows:
                row['multi_disc']=True
    for row in plans:
        if row['blocked'] or 'tags' not in row:continue
        effective=dict(row['tags'],**row['changes'])
        root,path=Path(row['root']),Path(row['path'])
        try:
            expected=destination(root,effective,row.get('multi_disc',False),path,row.get('layout'))
            row['layout_target']=str(expected)
            if expected!=path:
                row['issues'].append('Folder or filename differs from tag layout')
                if organise: row['target']=str(expected)
        except ValueError as exc:
            row['issues'].append(str(exc))
    folders={}
    for row in plans:
        if 'tags' in row:folders.setdefault(str(Path(row['path']).parent),[]).append(row)
    for rows in folders.values():
        credits={first(r['tags'],'albumartist') for r in rows}
        slots=Counter((first(r['tags'],'discnumber'), first(r['tags'],'tracknumber').split('/')[0]) for r in rows)
        totals_by_slot={}
        for r in rows:
            slot=(first(r['tags'],'discnumber'), first(r['tags'],'tracknumber').split('/')[0])
            if slot[1] and slots[slot]>1:
                tot=first(r['tags'],'tracktotal') or first(r['tags'],'totaltracks')
                totals_by_slot.setdefault(slot,set()).add(tot)
        for row in rows:
            if len(credits)>1:row['issues'].append('Mixed album artists in this folder')
            slot=(first(row['tags'],'discnumber'),first(row['tags'],'tracknumber').split('/')[0])
            if duplicate_tracks and slot[1] and slots[slot]>1:
                totals=totals_by_slot.get(slot,set())
                if len(totals)>1 and '1' in totals:
                    row['issues'].append('Duplicate track number (Single and Album mixed in folder)')
                else:
                    row['issues'].append('Duplicate track number in this folder')
    validate_targets(plans)
    for row in plans:
        row['changes'].update(row.get('overrides',{}))
        if row['organise'] and row.get('tags'):
            try:row['target']=str(destination(row['root'],dict(row['tags'],**row['changes']),row.get('multi_disc',False),row['path'],row.get('layout')))
            except ValueError:pass
    superseded = detect_superseded_singles(plans)
    for row in plans:
        if row['path'] in superseded:
            row['superseded_by'] = superseded[row['path']]
            row['issues'].append(superseded[row['path']]['reason'])
    validate_targets(plans)
    return plans


def validate_targets(plans):
    counts=Counter(name_key(row['target']).casefold() for row in plans if row['target']!=row['path'])
    for row in plans:
        row['collision']=''
        target=Path(row['target'])
        if row['target']!=row['path'] and (counts[name_key(row['target']).casefold()]>1 or target.exists() or target.is_symlink()):
            row['collision']='Destination already exists or is shared by multiple files; move skipped'


def set_album_artist(plans, value):
    value=value.strip()
    if not value: raise ValueError('Enter an album artist')
    for row in plans:
        if 'tags' not in row:continue
        row['changes']['albumartist']=[value]
        row.setdefault('overrides',{})['albumartist']=[value]
        if row['organise']:
            row['target']=str(destination(row['root'],dict(row['tags'],**row['changes']),row.get('multi_disc',False),row['path'],row.get('layout')))


class MaintenanceCancelled(Exception):
    """Cancellation before publication leaves the original file untouched."""


def check_cancelled(cancel):
    if cancel():raise MaintenanceCancelled('Cancelled; original file retained')


def audio_digest(path,cancel=lambda:False):
    from .tag_io import audio_digest as native_digest
    return native_digest(path, lambda: check_cancelled(cancel))


def guarded_path(path, root):
    path=Path(path);root=Path(root)
    if not path.is_relative_to(root) or path.resolve()!=path or path==root:
        raise ValueError('Path is outside the library or follows a symbolic link')


def _cleanup_empty_parents(source: Path, root: Path):
    parent = source.parent
    junk_files = {'.ds_store', 'thumbs.db', '.directory', 'desktop.ini'}
    while parent != root and parent.is_relative_to(root):
        try:
            remaining = list(parent.iterdir())
            if all(p.is_file() and p.name.lower() in junk_files for p in remaining):
                for p in remaining:
                    try: p.unlink(missing_ok=True)
                    except OSError: pass
            parent.rmdir()
            parent = parent.parent
        except OSError:
            break


def apply_one(row, store, cancel=lambda:False):
    check_cancelled(cancel)
    source,target,root=map(Path,(row['path'],row['target'],row['root']))
    if row['blocked'] or row.get('collision'):raise ValueError(row['blocked'] or row['collision'])
    guarded_path(source,root);guarded_path(target,root)
    if fingerprint(source)!=tuple(row['stamp']):raise ValueError('File changed since preview; inspect again')
    if row.get('superseded_by'):
        if row.get('operation') != 'organise':
            raise ValueError('Removing redundant singles requires an Organise files preview.')
        replacement = row['superseded_by']
        retained = Path(replacement['album_path'])
        guarded_path(retained, root)
        if not replacement.get('album_stamp') or not retained.is_file() or fingerprint(retained) != tuple(replacement['album_stamp']):
            raise ValueError('The retained album track changed or disappeared. Refresh the preview; nothing removed.')
        from .release_matching import replacement_matches
        original=inspect_file(source,root);kept=inspect_file(retained,root)
        if (original.get('blocked') or kept.get('blocked') or not replacement_matches(original,kept)
                or fingerprint(source)!=tuple(row['stamp']) or fingerprint(retained)!=tuple(replacement['album_stamp'])):
            raise ValueError('Replacement performer credits, recording, version or duration cannot be verified. Original retained.')
        trash_file(source)
        with store.connect() as db:
            db.execute('UPDATE local_files SET present=0 WHERE path=?', (str(source),))
        _cleanup_empty_parents(source, root)
        return 'applied'
    if source!=target and target.exists():raise ValueError('Destination exists; nothing overwritten')
    if not row['changes'] and not row.get('artwork_change') and source==target:return 'unchanged'
    if source!=target and not row['changes'] and not row.get('artwork_change'):
        # Exclusive same-volume move: retain the inode and every byte, without
        # copying and hashing a large audio file twice. Cross-volume falls back.
        metadata=read_metadata(source)
        target.parent.mkdir(parents=True,exist_ok=True);guarded_path(target,root)
        if fingerprint(source)!=tuple(row['stamp']):raise ValueError('Source changed while preparing the move; original retained')
        try:os.link(source,target)
        except OSError as exc:
            if exc.errno not in (errno.EXDEV,errno.EOPNOTSUPP,errno.ENOTSUP,errno.EPERM):raise
        else:
            try:
                # Creating the link itself changes ctime, but not identity, size or mtime.
                if fingerprint(source)[:4]!=tuple(row['stamp'])[:4] or not os.path.samefile(source,target):
                    raise ValueError('Source changed while moving; original retained')
                source.unlink()
                _cleanup_empty_parents(source, root)
            except Exception:
                target.unlink();raise
            index_applied(store,source,target,root,metadata)
            return 'applied'
    temporary=None
    try:
        target.parent.mkdir(parents=True,exist_ok=True)
        guarded_path(target,root)
        fd,temporary=tempfile.mkstemp(prefix='.library-tags-',suffix='.flac',dir=target.parent);os.close(fd)
        from .file_services import copy_flac_verified
        digest=copy_flac_verified(source,temporary,lambda:check_cancelled(cancel))
        shutil.copystat(source,temporary)
        audio=FLAC(temporary)
        original_tags={k:list(v) for k,v in audio.tags.items()}
        pictures=[p.write() for p in audio.pictures]
        for key,value in row['changes'].items():
            if value:audio[key]=value
            elif key in audio:del audio[key]
        if row.get('artwork_change'):
            from .artwork import checked_picture
            retained=[p for p in audio.pictures if p.type!=3]
            audio.clear_pictures()
            for picture in retained:audio.add_picture(picture)
            audio.add_picture(checked_picture(row['artwork_change']))
            pictures=[p.write() for p in audio.pictures]
        if row['changes'] or row.get('artwork_change'):audio.save(temporary)
        checked=FLAC(temporary)
        expected=dict(original_tags,**row['changes'])
        expected={key:value for key,value in expected.items() if key not in row['changes'] or value}
        if dict(checked.tags)!=expected or [p.write() for p in checked.pictures]!=pictures or audio_digest(temporary,cancel)!=digest:
            raise ValueError('Verification failed; original file retained')
        metadata=read_metadata(temporary)
        if not first(expected,'title'):metadata['title']=target.stem
        with open(temporary,'rb') as f:os.fsync(f.fileno())
        if fingerprint(source)!=tuple(row['stamp']):raise ValueError('Source changed while applying; original retained')
        guarded_path(source,root);guarded_path(target,root)
        check_cancelled(cancel)
        if target==source:
            os.replace(temporary,source);temporary=None
        else:
            # Exclusive creation: never overwrite a newly appeared destination.
            os.link(temporary,target)
            try:
                source.unlink()
                _cleanup_empty_parents(source, root)
            except Exception:
                target.unlink();raise
        index_applied(store,source,target,root,metadata)
        return 'applied'
    finally:
        if temporary and Path(temporary).exists():Path(temporary).unlink()


class AppliedButNotIndexed(Exception):
    pass


def index_applied(store,source,target,root,metadata):
    try:
        stat=target.stat()
        with store.connect() as db:
            if source!=target:db.execute('UPDATE local_files SET present=0 WHERE path=?',(str(source),))
            db.execute('INSERT OR REPLACE INTO local_files VALUES(?,?,?,?,?,?,1)',
                       (str(target),str(root),stat.st_size,stat.st_mtime_ns,json.dumps(metadata),None))
    except Exception as exc:
        raise AppliedButNotIndexed('File updated, but index refresh failed; rescan this library') from exc


def apply_plans(plans,store,cancel=lambda:False,progress=lambda s:None,on_applied=lambda row:None):
    applied=failed=processed=0
    for index,row in enumerate(plans,1):
        if cancel():break
        tags=row.get('tags',{})
        name=' — '.join(filter(None,(first(tags,'albumartist'),first(tags,'title') or Path(row['path']).name)))
        action='Updating artwork' if row.get('artwork_change') else 'Updating tags: '+', '.join(row['changes']) if row.get('changes') else 'Organising file'
        progress(f'{action} · {index:,}/{len(plans):,} · {name}')
        try:row['result']=apply_one(row,store,cancel);applied+=row['result']=='applied'
        except MaintenanceCancelled:
            row['result']='Cancelled; original file retained'
            progress(row['result']);break
        except AppliedButNotIndexed as exc:row['result']=str(exc);applied+=1;failed+=1
        except Exception as exc:row['result']='Not applied: '+str(exc);failed+=1
        if row.get('result')=='applied':
            try:on_applied(row)
            except Exception as exc:
                row['result']='File updated; dependent refresh failed: '+str(exc);failed+=1
        processed+=1
        if row['result'] not in ('applied','unchanged'):progress(row['result'])
        try:
            with store.connect() as db:
                db.execute('CREATE TABLE IF NOT EXISTS maintenance_history(at TEXT,path TEXT,target TEXT,changes TEXT,result TEXT)')
                db.execute('INSERT INTO maintenance_history VALUES(?,?,?,?,?)',(now(),row['path'],row['target'],json.dumps(dict(row['changes'],**({'artwork':row['artwork_change']} if row.get('artwork_change') else {}))),row['result']))
        except Exception:
            progress('Could not save repair history; file result is shown in the preview. Rescan to reconcile the index.')
            break
    return f'{applied} files updated · {failed} failed/skipped · {len(plans)-processed} not processed'
