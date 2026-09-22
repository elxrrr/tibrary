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
    from .tag_io import MP4
    pictures = sorted((p for p in pictures if p.type==3 or isinstance(audio, MP4)), key=lambda p:p.width*p.height, reverse=True)
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
    tags = normalize(audio, metadata)
    audio['tidal_track_id'] = [str(track_id)]
    audio['tidal_album_id'] = [str(album_id)]
    return tags, audio
