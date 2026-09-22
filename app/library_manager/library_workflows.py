"""Independent Library Tools plans built from a shared, read-only inspection."""
import copy
import os
from pathlib import Path
from .maintenance import inspect_file, fingerprint, first, replan, validate_targets
from .enrichment import tag_aliases

OPERATIONS = ('summary', 'tags', 'metadata', 'artwork', 'organise', 'links')
LYRIC_TAGS = ('lyrics','unsyncedlyrics','syncedlyrics')
CORRECTION_TAGS = {'albumartist', 'album', 'date', 'originaldate', 'releasedate',
                   'tracknumber', 'discnumber', 'disctotal', 'totaldiscs', 'tracktotal', 'totaltracks',
                   'title','artist','genre','label','copyright','upc','composer','lyricist','producer',
                   'isrc','tidal_track_id','tidal_album_id','releasetype','url','arranger','conductor','engineer','mixer','remixer','performer'}
METADATA_LABELS = {'bpm':'BPM', 'initialkey':'musical key', 'genre':'genre', 'label':'label', 'isrc':'ISRC', 'upc':'UPC', 'copyright':'copyright', 'releasetype':'release type'}


def missing_metadata(tags):
    missing=[]
    for key,label in METADATA_LABELS.items():
        aliases = tag_aliases(key)
        if not any(first(tags,a) for a in aliases):missing.append(label)
    return missing


def inspect_snapshot(root, snapshot=(), cancel=lambda:False, progress=lambda s:None, layout=None, paths=None):
    """Reuse unchanged files; removed paths leave the inspection after a complete walk."""
    root=Path(root).resolve(strict=True)
    cached={r['path']:r for r in snapshot}
    if paths is None:
        from .file_services import inventory
        entries, complete = inventory(root, {'.flac'}, cancel, progress)
        if not complete:return list(snapshot)
        paths=[path for path, _, _ in entries]
    result=[];read=0
    for path in paths:
        if cancel():return list(snapshot)
        old=cached.get(str(path))
        try:unchanged=old and not old.get('blocked') and tuple(old.get('stamp',()))==fingerprint(Path(path))
        except OSError:unchanged=False
        if unchanged:row=copy.deepcopy(old)
        else:row=inspect_file(path,root,layout);read+=1
        row.pop('result',None);row['layout']=layout or {};result.append(row)
        if len(result)%100==0:progress(f'Checking library health · {len(result):,} files · {read:,} tags reread')
    progress(f'Inspection current · {len(result):,} FLAC files · {read:,} tags read · {len(result)-read:,} reused')
    return result


def workflow_plan(snapshot, operation, dates=True, discs=True, remove_lyrics=False, organise_local=False, keys=True, stray=False, duplicate_tracks=False, arrange_layout=True, superseded_singles=False):
    if operation not in OPERATIONS:raise ValueError('Unknown library operation')
    clean=copy.deepcopy(snapshot)
    for row in clean:
        row.pop('result',None)
        row['enrich_enabled']=operation=='metadata'
        row['covers_enabled']=operation=='artwork'
        row['overrides']={k:v for k,v in row.get('overrides',{}).items()
                          if operation in ('tags','links') and (k in CORRECTION_TAGS or k=='artist')}
        if operation!='links':row.pop('catalogue_choice',None)
        elif row.get('catalogue_choice'):
            row['catalogue_choice']['changes']={k:v for k,v in row['catalogue_choice']['changes'].items() if k in CORRECTION_TAGS}
        row.pop('catalogue_note',None)
    
    is_organise = (operation == 'organise')
    should_arrange = (is_organise and arrange_layout) or (operation == 'tags' and organise_local)
    check_duplicates = duplicate_tracks if (is_organise or operation == 'tags') else False

    planned=replan(clean,repair=operation=='links',dates=operation=='tags' and dates,
                   organise=should_arrange,discs=operation=='tags' and discs,
                   duplicate_tracks=check_duplicates)
    strays_map = {}
    if (is_organise or operation == 'tags') and stray:
        from .maintenance import stray_changes
        strays_map = stray_changes(snapshot)
    for source,row in zip(snapshot,planned):
        if operation != 'organise' or not superseded_singles:
            row.pop('superseded_by', None)
        # Preserve proposals for the other pages without including them in this operation.
        for key in ('overrides','catalogue_choice','catalogue_options','catalogue_note','metadata_changes','metadata_note','artwork_proposal','artwork_note','layout_target'):
            if key in source:row[key]=copy.deepcopy(source[key])
        if source.get('artwork_change'):row['artwork_proposal']=copy.deepcopy(source['artwork_change'])
        tags=row.get('tags',{})
        if operation=='tags' and keys:
            from .musical_keys import key_changes
            edits,issue=key_changes(tags);row['changes'].update(edits)
            if issue:row['issues'].append(issue)
        if (is_organise or (operation=='tags' and organise_local)) and stray and row['path'] in strays_map:
            edits, note = strays_map[row['path']]
            row['changes'].update(edits)
            row['issues'].append(note)
            row['organise'] = True
            row['stray_reunited'] = True
            from .maintenance import destination
            try:
                row['target'] = str(destination(row['root'], dict(row['tags'], **row['changes']), row.get('multi_disc', False), row['path'], row.get('layout')))
                row['layout_target'] = row['target']
            except ValueError:
                pass
        row['remove_lyrics']=operation=='tags' and remove_lyrics
        if row['remove_lyrics']:
            row['changes'].update({key:[] for key in LYRIC_TAGS if key in tags})
        if operation=='metadata':
            row['issues']=['Missing '+label for label in missing_metadata(tags)]
            if row.get('metadata_note'):row['issues'].append(row['metadata_note'])
        elif operation=='artwork':
            size=row.get('cover_size')
            row['issues']=[] if size==(1280,1280) else [f'Front cover: {size[0]} × {size[1]} → target 1280 × 1280' if size else 'No readable front cover']
            if row.get('artwork_note'):row['issues'].append(row['artwork_note'])
        elif operation=='organise':
            if not (is_organise and superseded_singles):
                row.pop('superseded_by', None)
                row['issues'] = [iss for iss in row['issues'] if not iss.startswith('Superseded single')]
            if not should_arrange and row['path'] not in strays_map and not row.get('superseded_by'):
                row['target'] = row['path']
            row['issues']=[issue for issue in row['issues'] if any(s in issue for s in ('folder','Folder','filename','path','layout','before moving','Missing layout','track number is required','Duplicate track','Superseded single','Stray track'))]
        elif operation in ('tags','links'):
            if operation == 'tags':
                row.pop('superseded_by', None)
                if not organise_local:
                    row['target'] = row['path']
            row['issues']=[issue for issue in row['issues'] if not any(s in issue for s in ('Missing BPM','Missing musical key','artwork','Front cover','ISRC','Folder or filename','Artist folder','Superseded single','Duplicate track'))]
            if operation=='links' and row.get('catalogue_options') and not row.get('catalogue_choice') and any(o.get('changes') for o in row['catalogue_options']):
                row['issues'].append('Verified album credits need your choice · use Review one album at a time')
            if operation=='links' and not row.get('linked_ids') and not (first(tags,'tidal_track_id') and first(tags,'tidal_album_id')):
                row['issues'].append('Recording and release not linked · Start / resume linking')
            if operation=='links':
                if row.get('linked_ids'):
                    incomplete = [c.get('note') for c in row.get('dj_checks', []) if c.get('status') == 'incomplete']
                    row['issues'] = incomplete if incomplete else []
                else:
                    row['issues'] = [row.get('catalogue_note') or row.get('link_note') or 'Not checked · Start / resume linking']
            elif operation == 'tags':
                from .maintenance import date_changes, disc_changes, track_changes
                d_edits, _ = date_changes(tags)
                disc_edits, _ = disc_changes(tags)
                from .musical_keys import key_changes
                k_edits, _ = key_changes(tags)
                if d_edits:
                    row['issues'].append('Date formatting unstandardised')
                if disc_edits or track_changes(tags)[0]:
                    row['issues'].append('Track/disc numbers unstandardised')
                if k_edits:
                    row['issues'].append('Musical key not Camelot')
                if any(key in tags for key in LYRIC_TAGS):
                    if remove_lyrics:
                        row['issues'].append('Remove existing lyrics tags')
                    else:
                        row['issues'].append('Contains lyrics tags')
                if organise_local and row['target']!=row['path']:
                    row['issues'].append('Folder or filename differs from tag layout')
        elif operation == 'summary':
            from .maintenance import date_changes, disc_changes, track_changes
            from .musical_keys import key_changes
            d_edits, _ = date_changes(tags)
            disc_edits, _ = disc_changes(tags)
            k_edits, _ = key_changes(tags)
            if d_edits: row['issues'].append('Date formatting unstandardised')
            if disc_edits or track_changes(tags)[0]: row['issues'].append('Track/disc numbers unstandardised')
            if k_edits: row['issues'].append('Musical key not Camelot')
            missing = missing_metadata(tags)
            if missing: row['issues'].extend(['Missing ' + m for m in missing])
            cov = row.get('cover_size')
            if cov and cov != (1280, 1280): row['issues'].append(f'Front cover: {cov[0]} × {cov[1]} → 1280 × 1280')
            elif not cov: row['issues'].append('No readable front cover')
            if row.get('target') and row['target'] != row['path']: row['issues'].append('Folder/filename differs from tag layout')
            if row.get('superseded_by'): row['issues'].append('Superseded single')
        row['issues'] = list(dict.fromkeys(row['issues']))
        row['operation']=operation
    validate_targets(planned)
    return planned


def has_changes(row):
    return bool(row.get('changes') or row.get('artwork_change') or row.get('target',row['path'])!=row['path'] or row.get('superseded_by'))


def attention(row):
    return bool(row.get('blocked') or row.get('collision') or row.get('apply_error') or row.get('issues') or has_changes(row))


def validate_operation(row, operation):
    """Check the operation boundary again immediately before handing a plan to the writer."""
    if row.get('operation')!=operation:raise ValueError('The selected plan belongs to another operation. Refresh the preview.')
    moved=row['path']!=row['target'];changes=row.get('changes',{});cover=row.get('artwork_change')
    if operation=='organise':
        allowed_changes = {'albumartist'} if row.get('stray_reunited') else set()
        if set(changes) - allowed_changes or cover:
            raise ValueError('An organisation plan must contain file moves and superseded single removals only.')
    elif moved and not (operation=='tags' and row.get('organise')):
        raise ValueError('Tag and artwork operations must keep files in place.')
    if operation in ('tags','links','metadata') and cover:raise ValueError('Artwork changes belong to the Artwork page.')
    if operation=='artwork' and changes:raise ValueError('Artwork plans cannot change tags.')
    if operation=='tags':
        allowed={'date','originaldate','releasedate','discnumber','disctotal','totaldiscs','tracknumber','tracktotal','totaltracks','initialkey','key','tkey','albumartist'} | set(row.get('overrides',{})) | set(LYRIC_TAGS)
        if set(changes)-allowed:raise ValueError('Online tag corrections belong to Link Releases.')
        if row.get('superseded_by'):raise ValueError('Superseded single removals belong to Organise files.')
    for key in LYRIC_TAGS:
        if key in changes and not (operation=='tags' and row.get('remove_lyrics') and changes[key]==[]):
            raise ValueError('Lyrics may only be removed with the explicit Correct tags option.')
    if operation=='metadata':
        for key in changes:
            if key in ('lyrics','unsyncedlyrics'):raise ValueError('Lyrics are excluded from metadata enrichment.')
            aliases=('initialkey',) if key=='initialkey' else tag_aliases(key)
            if any(first(row.get('tags',{}),alias) for alias in aliases):raise ValueError('Missing-tag plans cannot replace existing values.')
