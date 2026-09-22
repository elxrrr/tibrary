"""Unambiguous notation changes only: never infer or change a track's musical key."""
import re
KEY_TAGS=('initialkey','key','tkey')
_MINOR=('G#m','D#m','A#m','Fm','Cm','Gm','Dm','Am','Em','Bm','F#m','C#m')
_MAJOR=('B','F#','C#','G#','D#','A#','F','C','G','D','A','E')
_FLATS={'Cb':'B','Db':'C#','Eb':'D#','Fb':'E','Gb':'F#','Ab':'G#','Bb':'A#','B#':'C','E#':'F'}


def canonical_key(value):
    text=str(value).strip().replace('♯','#').replace('♭','b')
    camelot=re.fullmatch(r'(1[0-2]|[1-9])([AB])',text,re.I)
    if camelot:return (_MINOR if camelot[2].upper()=='A' else _MAJOR)[int(camelot[1])-1]
    text=re.sub('sharp','#',text,flags=re.I);text=re.sub('flat','b',text,flags=re.I)
    text=re.sub(r'(?i)(major|maj|minor|min)$',lambda m:m[0].lower(),text)
    match=re.fullmatch(r'([A-Ga-g])([#b]?)\s*(major|maj|minor|min|m)?',text)
    if not match:return None
    note=match[1].upper()+match[2]
    return _FLATS.get(note,note)+('m' if match[3] in ('minor','min','m') else '')


def camelot_key(value):
    note=canonical_key(value)
    if note in _MINOR:return str(_MINOR.index(note)+1)+'A'
    if note in _MAJOR:return str(_MAJOR.index(note)+1)+'B'
    return None


def supplied_key(track):
    """Only use explicitly named key fields; never infer from titles or arbitrary tags."""
    values=[]
    for field in ('initialkey','tkey','musical_key','key'):
        value=track.get(field)
        if value in (None,'','UNKNOWN'):continue
        if field=='key' and 'key_scale' in track:
            if track.get('key_scale') not in ('MAJOR','MINOR'):continue
            value=re.sub('sharp','#',str(value),flags=re.I)
            value=re.sub('flat','b',value,flags=re.I)
            if re.fullmatch(r'[A-Ga-g][#b♯♭]?', str(value).strip()):
                value=str(value)+('m' if track['key_scale']=='MINOR' else '')
        converted=camelot_key(value)
        if converted:values.append(converted)
    return values[0] if values and len(set(values))==1 else None


def key_changes(tags):
    present={k:v for k,v in tags.items() if k in KEY_TAGS and v}
    if not present:return {},''
    values={camelot_key(v) for group in present.values() for v in group}
    if None in values or len(values)!=1:return {},'Musical key notation is unknown or conflicting; unchanged'
    value=[values.pop()]
    # INITIALKEY is the canonical destination; source tags remain intact.
    return ({'initialkey':value} if tags.get('initialkey')!=value else {}),''
