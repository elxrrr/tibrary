"""Normalize staged downloads only; existing library files are never opened for writes."""
import io
from pathlib import Path
from .maintenance import normalized_date
from .organisation import layout_path

DOWNLOAD_LAYOUT = '{albumartist}/{album} ({year})/{disc}/{tracknumber} - {title}'


def normalize(audio, metadata=None):
    from .musical_keys import key_changes
    metadata = metadata or {}
    for key, values in metadata.items():
        if values: audio[key] = [str(v) for v in values]
    tags = {k: list(v) for k,v in audio.tags.items()}
    for kind in ('track', 'disc'):
        parts = str(tags.get(kind+'number', [''])[0]).split('/')
        raw_total = tags.get(kind+'total', tags.get('total'+kind+'s', [parts[1] if len(parts)>1 else '']))[0]
        num = parts[0]
        if not num.isdecimal() or int(num)<1: raise ValueError(f'Download is missing a valid {kind} number')
        if not str(raw_total).isdecimal() or int(raw_total)<int(num): raise ValueError(f'Download is missing a valid {kind} total')
        audio[kind+'number'] = [f'{int(num):02d}']
        audio[kind+'total'] = [f'{int(raw_total):02d}']
        if 'total'+kind+'s' in audio: audio['total'+kind+'s'] = [f'{int(raw_total):02d}']
    for key in ('date', 'originaldate', 'releasedate'):
        if key in audio:
            date = normalized_date(audio[key][0])
            if not date:raise ValueError(f'Download has an invalid {key}; review its metadata')
            audio[key] = [date if len(date)==10 else date[:4]]
    edits, _ = key_changes(tags)
    for key, values in edits.items(): audio[key] = values
    from .library_workflows import LYRIC_TAGS
    for key in list(audio.keys()):
        if key.lower() in LYRIC_TAGS: del audio[key]
    for key in ('albumartist','artist'):
        if not audio.get(key): raise ValueError(f'Download is missing {key} credits')
    return {k:list(v) for k,v in audio.tags.items()}


def companion_cover(audio, album_root):
    pictures = getattr(audio, 'pictures', [])
    pictures = sorted((p for p in pictures if p.type==3), key=lambda p:p.width*p.height, reverse=True)
    data = pictures[0].data if pictures else next(iter((audio.tags or {}).get('covr',[])),None)
    if data is None:return
    from PIL import Image
    with Image.open(io.BytesIO(data)) as image:
        if image.width*image.height > 40_000_000: raise ValueError('Cover image is too large')
        data = io.BytesIO(); image.convert('RGB').save(data, format='JPEG', quality=95)
    target = Path(album_root)/'cover.jpg'
    if not target.exists():
        with target.open('xb') as stream: stream.write(data.getvalue())


def download_path(stage, tags, template=None, extension='.flac'):
    date = tags.get('date', [''])[0]
    relevant = int(tags['disctotal'][0]) > 1
    return layout_path(stage, tags, date[:4] if date[:4].isdecimal() else '', relevant,
                       template=template or DOWNLOAD_LAYOUT, disc_padding=False, extension=extension)


def normalize_m4a(audio, metadata, track_id, album_id):
    """Keep the existing AAC quality options working; MP4 stores totals as tuples."""
    class TagMap(dict):
        @property
        def tags(self):return self
    from mutagen.easymp4 import EasyMP4Tags
    from mutagen.mp4 import MP4,MP4FreeForm
    raw=MP4(audio.filename)
    bag=TagMap({k:list(v) for k,v in audio.tags.items()})
    key_atom='----:com.apple.iTunes:initialkey'
    if raw.get(key_atom):bag['initialkey']=[bytes(v).decode('utf-8') for v in raw[key_atom]]
    tags=normalize(bag,metadata)
    for key,values in tags.items():
        if key in EasyMP4Tags.Set:audio[key]=values
    for kind in ('track','disc'):audio[kind+'number']=[tags[kind+'number'][0]+'/'+tags[kind+'total'][0]]
    audio.save()
    raw=MP4(audio.filename)
    for key,values in dict(tidal_track_id=[track_id],tidal_album_id=[album_id],initialkey=tags.get('initialkey',[])).items():
        if values:raw['----:com.apple.iTunes:'+key]=[MP4FreeForm(str(v).encode('utf-8')) for v in values]
    raw.pop('©lyr',None);raw.save()
    return tags,raw
