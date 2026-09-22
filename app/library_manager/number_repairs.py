"""Preview number repairs from local tags and cached, verified edition evidence."""
from collections import defaultdict
from .release_matching import group_key,position,total
from .core import title_key


def number_repairs(rows):
    from .maintenance import disc_changes,track_changes,first
    groups=defaultdict(list)
    for row in rows:
        if not row.get('blocked') and row.get('tags'):groups[group_key(row)].append(row)
    results={}
    for group in groups.values():
        max_disc=max(position(r)[0] for r in group)
        for row in group:
            tags=row['tags'];disc,index=position(row)
            peers=[r for r in group if position(r)[0]==disc]
            max_track=max(position(r)[1] for r in peers)
            repaired=dict(tags);notes=[]
            for kind,minimum,siblings in (('track',max_track,peers),('disc',max_disc,group)):
                current=total(row,kind)
                if current and current>=minimum:continue
                local={total(r,kind) for r in siblings if total(r,kind)>=minimum and total(r,kind)>0}
                online=set()
                options=row.get('catalogue_options') or ([row['catalogue_choice']] if row.get('catalogue_choice') else [])
                for option in options:
                    info=option.get('structure',{})
                    if not info.get('compatible') or not (option.get('recording_verified') or option.get('whole_release_verified')):continue
                    if title_key(option.get('album',''))!=title_key(first(tags,'album')):continue
                    alignment=info.get('alignments',{}).get(row['path'],{})
                    if (alignment.get('disc_number'),alignment.get('track_number'))!=(disc,index):continue
                    proposed=int((option.get('release_counts') or {}).get(kind+'total') or 0) or total({'tags':option.get('changes',{})},kind)
                    if proposed>=minimum and proposed>0:online.add(proposed)
                evidence=local|online
                if len(evidence)==1:
                    value=next(iter(evidence));repaired[kind+'total']=[f'{value:02d}']
                    if 'total'+kind+'s' in tags:repaired['total'+kind+'s']=[f'{value:02d}']
                    # An embedded /total must agree with the repaired separate total.
                    raw=first(tags,kind+'number')
                    if '/' in raw:repaired[kind+'number']=[raw.split('/')[0]]
                    notes.append(f'{kind.capitalize()} total → {value:02d} · '+('verified cached release' if online else 'consistent local release tags'))
                elif len(evidence)>1:notes.append(f'Conflicting {kind} totals in local/cached release evidence; review required')
            edits={k:v for k,v in repaired.items() if tags.get(k)!=v}
            for normalize in (disc_changes,track_changes):
                changes,issue=normalize(repaired);edits.update(changes)
                if issue:notes.append(issue)
            results[row['path']]=(edits,notes)
    return results
