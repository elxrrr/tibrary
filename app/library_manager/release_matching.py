"""Shared, conservative recording and release structure evidence (no I/O)."""
import re
import math
from pathlib import Path
from .core import title_key


def value(row, *keys):
    data = row.get('tags', row)
    for key in keys:
        v = data.get(key)
        if isinstance(v, (list, tuple)): v = v[0] if v else ''
        if v is not None and str(v).strip(): return str(v).strip()
    return ''


def number(row, *keys, default=0):
    raw = value(row, *keys).split('/')[0]
    return int(raw) if raw.isdecimal() else default


def total(row, kind):
    explicit = number(row, kind+'total', 'total'+kind+'s', 'total_'+kind+'s')
    tail = value(row, kind+'number', kind).partition('/')[2]
    return explicit or (int(tail) if tail.isdecimal() else 0)


def position(row):
    return (number(row, 'disc_number', 'discnumber', 'disc', default=1),
            number(row, 'track_number', 'tracknumber', 'track'))


def release_folder(row):
    p = Path(row['path']).parent
    if re.fullmatch(r'(?:disc|cd)\s*0*\d+', p.name, re.I): p = p.parent
    return p


def group_key(row):
    return (str(release_folder(row)), title_key(value(row, 'albumartist', 'artist')),
            title_key(value(row, 'album')))


def recording_matches(local, remote, strict=False):
    """Known conflicts always win over identifiers; strict requires timing and title."""
    lt, rt = value(local, 'title'), value(remote, 'title')
    def duration(row):
        try:v=float(row.get('duration') or 0)
        except (ValueError,TypeError):return 0
        return v if math.isfinite(v) and v>0 else 0
    ld, rd = duration(local),duration(remote)
    if ld and rd and abs(ld-rd) > 3: return False
    li, ri = (re.sub(r'[^A-Z0-9]', '', value(r, 'isrc').upper()) for r in (local, remote))
    if li and ri and li != ri: return False
    # Mix names must agree even if a provider reused an ISRC.
    mix = r'\b(?:remix|mix|extended|instrumental|radio|club|live|edit|acoustic)\b'
    if lt and rt and title_key(lt) != title_key(rt) and (re.search(mix, lt, re.I) or re.search(mix, rt, re.I)):
        return False
    if strict and (not ld or not rd or not lt or not rt or title_key(lt) != title_key(rt)): return False
    if li and ri: return li == ri
    tid = value(local, 'tidal_track_id', 'tidaltrackid') or str(local.get('linked_ids', {}).get('track_id') or '')
    if tid and tid == str(remote.get('id')): return True
    return bool(lt and rt and title_key(lt)==title_key(rt) and ld and rd)


def replacement_matches(source, replacement):
    """Stricter evidence for retiring audio than for proposing a catalogue link.

    Missing performer credits are uncertainty, never permission to discard a
    file. Preserve punctuation inside names; ambiguous credit formatting can
    remain for manual inspection rather than guessing who performed a track.
    """
    import unicodedata
    def credits(row):
        tags=row.get('tags')
        raw=(tags.get('artist') if tags is not None else
             row.get('track_artist',row.get('artists')))
        if not raw:return set()
        if not isinstance(raw,(list,tuple)):raw=[raw]
        result=set()
        for item in raw:
            if isinstance(item,dict):item=item.get('name','')
            for name in re.split(r'[,;]|\s+(?:feat\.?|ft\.?|featuring)\s+',str(item),flags=re.I):
                name=' '.join(unicodedata.normalize('NFC',name).casefold().split())
                if name:result.add(name)
        return result
    left,right=credits(source),credits(replacement)
    if not left or left!=right:return False
    # Do not erase remasters, edits or mixes through title normalization.
    def title(row):return ' '.join(unicodedata.normalize('NFC',value(row,'title')).casefold().split())
    if not title(source) or title(source)!=title(replacement):return False
    isrcs=[re.sub(r'[^A-Z0-9]','',value(row,'isrc').upper()) for row in (source,replacement)]
    return bool(all(isrcs) and isrcs[0]==isrcs[1] and recording_matches(source,replacement,strict=True))


def omitted_version_alignments(rows, release):
    """Recover omitted live-version labels only with complete edition evidence.

    This is linking evidence, never a relaxation of replacement/pruning rules.
    Every file must have the exact recording ID, position and duration of the
    same named complete release. Conflicting explicit mix labels still fail.
    """
    tracks=release.get('tracks',[])
    if not rows or len(rows)!=len(tracks):return {}
    indexed={position(t):t for t in tracks}
    if len(indexed)!=len(tracks) or len({position(r) for r in rows})!=len(rows):return {}
    recovered={}
    for row in rows:
        track=indexed.get(position(row))
        if not track or title_key(value(row,'album'))!=title_key(release.get('title','')):return {}
        ids=[re.sub(r'[^A-Z0-9]','',value(t,'isrc').upper()) for t in (row,track)]
        try:durations=[float(t.get('duration') or 0) for t in (row,track)]
        except (TypeError,ValueError):return {}
        if not all(ids) or ids[0]!=ids[1] or not all(math.isfinite(d) and d>0 for d in durations) or abs(durations[0]-durations[1])>3:return {}
        if recording_matches(row,track):continue
        title=value(track,'title')
        bare=re.sub(r'\s*\(Live(?:\s+[^)]*)?\)\s*$','',title,flags=re.I)
        if bare==title or title_key(value(row,'title'))!=title_key(bare):return {}
        recovered[row.get('path','')]=str(track.get('id',''))
    return recovered


def structure(rows, release):
    """Compare a local release with one complete catalogue track list.

    FLAC writers disagree about whether TRACKTOTAL means the current disc or
    the complete release, and damaged downloads sometimes contain ``1`` in
    every total field.  Totals therefore support both conventions.  Stable
    recording evidence may also repair a wrong local position when that
    recording occurs exactly once on the candidate release.
    """
    recovered=omitted_version_alignments(rows,release)
    def matches(row,track):
        return recording_matches(row,track) or recovered.get(row.get('path',''))==str(track.get('id',''))
    tracks = release.get('tracks', [])
    positions = {position(t): t for t in tracks}
    counts = {}
    for disc, track in positions:
        counts[disc] = counts.get(disc, 0)+1
    disc_count = release.get('disc_count') or max(counts, default=0)
    conflicts = []
    if len(positions)!=len(tracks):conflicts.append('Catalogue track positions are incomplete or duplicated')
    if release.get('track_count') is not None and int(release['track_count'])!=len(tracks):conflicts.append('Catalogue track list is incomplete')
    claimed = set(); alignments = {}; unmatched = []
    # Keep exact position matches first. This makes duplicate titles on an
    # album deterministic before the unique-recording recovery pass below.
    for row in rows:
        declared = position(row); other = positions.get(declared)
        if declared[1] and other is not None and matches(row, other) and declared not in claimed:
            claimed.add(declared);alignments[row.get('path','')]=dict(
                disc_number=declared[0],track_number=declared[1],track_id=str(other.get('id') or ''))
        else:
            unmatched.append(row)
    for row in unmatched:
        hits = [p for p,t in positions.items() if p not in claimed and matches(row,t)]
        if len(hits)==1:
            matched=hits[0];claimed.add(matched);track=positions[matched]
            alignments[row.get('path','')]=dict(disc_number=matched[0],track_number=matched[1],
                                                track_id=str(track.get('id') or ''))
        else:
            conflicts.append('Track position or recording differs' if position(row)[1] else 'Track position is unverified')

    # Judge totals after alignment. A total smaller than an observed local
    # position/file count is malformed local metadata, so it is repairable
    # rather than evidence against an otherwise exact release.
    local_disc_counts = {}
    local_disc_max = {}
    for row in rows:
        disc,index=position(row)
        local_disc_counts[disc]=local_disc_counts.get(disc,0)+1
        local_disc_max[disc]=max(local_disc_max.get(disc,0),index)
    local_max_disc=max((position(row)[0] for row in rows),default=1)
    total_repairs=[]
    for row in rows:
        declared_disc,declared_track=position(row)
        aligned=alignments.get(row.get('path',''),{})
        disc=int(aligned.get('disc_number') or declared_disc or 1)
        expected_tracks,expected_discs=total(row,'track'),total(row,'disc')
        valid_track_totals={len(tracks),counts.get(disc,0)}-{0}
        malformed_track_total=bool(expected_tracks and (
            expected_tracks < declared_track or expected_tracks < local_disc_counts.get(declared_disc,0)
            or expected_tracks < local_disc_max.get(declared_disc,0)))
        if expected_tracks and expected_tracks not in valid_track_totals:
            if malformed_track_total:total_repairs.append(row.get('path',''))
            else:conflicts.append('Track total differs')
        malformed_disc_total=bool(expected_discs and (expected_discs < declared_disc or expected_discs < local_max_disc))
        if expected_discs and disc_count and expected_discs != int(disc_count):
            if malformed_disc_total:total_repairs.append(row.get('path',''))
            else:conflicts.append('Disc total differs')
    missing = [t for t in tracks if position(t) not in claimed]
    declared_positions={row.get('path',''):position(row) for row in rows}
    position_repairs=[path for path,matched in alignments.items()
                      if (matched.get('disc_number'),matched.get('track_number'))!=declared_positions.get(path)]
    return dict(omitted_version_labels=recovered,compatible=not conflicts and bool(tracks), conflicts=sorted(set(conflicts)),
                missing=missing, incomplete=not conflicts and bool(claimed) and bool(missing),
                matched=len(claimed), total=len(tracks), alignments=alignments,
                position_repairs=position_repairs,total_repairs=sorted(set(total_repairs)))
