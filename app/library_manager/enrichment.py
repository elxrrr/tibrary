"""Only propose explicitly supplied metadata for a verified recording."""
import math
from .maintenance import first, normalized_date


def copyright_text(value):
    return value.get('text','') if isinstance(value,dict) else value if isinstance(value,str) else ''


def tag_aliases(key):
    for group in (('bpm','tempo'),('initialkey','key','tkey'),('upc','barcode'),
                  ('label','recordlabel'),('lyrics','unsyncedlyrics'),
                  ('disctotal','totaldiscs'),('tracktotal','totaltracks')):
        if key in group:return group
    return (key,)


def needs_extended_metadata(tags,release,track):
    """Only consult python-tidal for fields that its metadata models can supply."""
    supplied=missing_tags(tags,release,track)
    fields=('bpm','initialkey','upc','copyright','releasetype','date','album',
            'albumartist','title','artist','isrc','disctotal')
    return any(not supplied.get(key) and not any(first(tags,a) for a in tag_aliases(key)) for key in fields)


def corrected_tags(tags,release,track):
    """Compare supplied catalogue values; retain local DJ analysis and unprovided tags."""
    supplied=missing_tags({},release,track)
    identifiers={'tidal_track_id','tidal_album_id','isrc'}
    protected={'bpm','initialkey','key','tkey','tempo'}
    return {key:values for key,values in supplied.items()
            if key not in protected and (first(tags,key) or key in identifiers) and tags.get(key)!=values}


def missing_tags(tags, release, track):
    values=dict(title=[track.get('title','')],artist=track.get('artists',[]),album=[release.get('title','')],
                albumartist=release.get('album_artists',[]),isrc=[track.get('isrc') or ''],
                genre=track.get('genres') or release.get('genres') or [],
                label=[release.get('label',{}).get('name','') if isinstance(release.get('label'),dict) else release.get('label') or ''],
                copyright=[copyright_text(track.get('copyright')) or copyright_text(release.get('copyright'))],
                upc=[release.get('barcode') or ''],tidal_track_id=[str(track['id'])],tidal_album_id=[str(release['id'])])
    release_type=release.get('type')
    if isinstance(release_type,str) and release_type.lower() in ('album','single','ep','compilation'):values['releasetype']=[release_type.lower()]
    if str(track['id']).isdecimal():values['url']=[f'https://tidal.com/track/{track["id"]}']
    if normalized_date(release.get('date','')):values['date']=[release['date']]
    for tag,key in [('tracknumber','track_number'),('discnumber','disc_number')]:
        if track.get(key):values[tag]=[f'{int(track[key]):02d}']
    if release.get('disc_count'):values['disctotal']=[f"{int(release['disc_count']):02d}"]
    if track.get('disc_number'):
        on_disc=[t for t in release.get('tracks',[]) if t.get('disc_number')==track['disc_number']]
        if on_disc:values['tracktotal']=[f'{len(on_disc):02d}']
    try:bpm=float(track.get('bpm'))
    except (TypeError,ValueError):bpm=0
    if not (first(tags,'bpm') or first(tags,'tempo')) and math.isfinite(bpm) and bpm>0:values['bpm']=[str(track['bpm'])]
    from .musical_keys import supplied_key
    if not first(tags,'initialkey'):
        value=supplied_key(track)
        if value:
            from .musical_keys import camelot_key
            existing=[first(tags,k) for k in ('key','tkey') if first(tags,k)]
            if all(camelot_key(v)==value for v in existing):values['initialkey']=[value]
    for role,tag in [(r,r) for r in ('composer','lyricist','producer','arranger','conductor','engineer','mixer','remixer','performer')]:
        names=[c['name'] for c in track.get('credits',[]) if c.get('role','').casefold()==role and c.get('name')]
        if names:values[tag]=list(dict.fromkeys(names))
    return {k:list(dict.fromkeys(str(v).strip() for v in vals if str(v).strip())) for k,vals in values.items()
            if not any(first(tags,a) for a in (('initialkey',) if k=='initialkey' else tag_aliases(k))) and any(str(v).strip() for v in vals)}
