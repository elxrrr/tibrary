"""Normalize staged downloads only; existing library files are never opened for writes."""
import io
import re
import string
import unicodedata
from datetime import date
from pathlib import Path

DOWNLOAD_LAYOUT = '{albumartist}/{album} ({year})/{disc}/{tracknumber} - {title}'
LYRIC_TAGS = {
    'lyrics',
    'unsyncedlyrics',
    'unsynced lyrics',
    'unsynced_lyrics',
    'synced lyrics',
    'synced_lyrics',
}

CAMELOT_MAP = {
    'Abm': '1A', 'B': '1B', 'Ebm': '2A', 'F#': '2B', 'Gb': '2B',
    'Bbm': '3A', 'Db': '3B', 'C#': '3B', 'Fm': '4A', 'Ab': '4B',
    'G#': '4B', 'Cm': '5A', 'Eb': '5B', 'D#': '5B', 'Gm': '6A',
    'Bb': '6B', 'A#': '6B', 'Dm': '7A', 'F': '7B', 'Am': '8A',
    'C': '8B', 'Em': '9A', 'G': '9B', 'Bm': '10A', 'D': '10B',
    'F#m': '11A', 'Gbm': '11A', 'A': '11B', 'C#m': '12A', 'Dbm': '12A',
    'E': '12B',
}


def normalized_date(value):
    value = re.sub(r'[T\s]\d{2}:\d{2}:\d{2}.*$', '', str(value).strip())
    if re.fullmatch(r'\d{4}/\d{2}/\d{2}', value):
        value = value.replace('/', '-')
    try:
        if re.fullmatch(r'\d{4}', value):
            date(int(value), 1, 1)
        elif re.fullmatch(r'\d{4}-\d{2}', value):
            date.fromisoformat(value + '-01')
        elif re.fullmatch(r'\d{4}-\d{2}-\d{2}', value):
            date.fromisoformat(value)
        else:
            return None
        return value
    except ValueError:
        return None


def safe_component(value, final=True):
    raw = unicodedata.normalize('NFC', str(value))

    def separator(match):
        left = raw[:match.start()].rstrip()
        right = raw[match.end():].lstrip()
        return ' - ' if left and right and left[-1] not in '([{' and right[0] not in ')]}' else ''

    cleaned = re.sub(r'\s*[/\\:|]+\s*', separator, raw)
    cleaned = re.sub(r'[\x00-\x1f\x7f*?"<>]', '', cleaned).strip()
    if cleaned != raw:
        cleaned = re.sub(' {2,}', ' ', cleaned)
    if final:
        cleaned = cleaned.rstrip(' .')
    if cleaned in ('', '.', '..'):
        raise ValueError('Tags produce an empty or unsafe filename component')
    if final and re.match(r'^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)', cleaned, re.I):
        cleaned = '_' + cleaned
    return cleaned


def layout_path(root, tags, year='', disc_relevant=False, template=DOWNLOAD_LAYOUT, extension='.flac', disc_padding=False):
    values = {k.lower(): str(v[0]).strip() for k, v in tags.items() if v}
    for key in ('albumartist', 'artist'):
        if tags.get(key):
            values[key] = ', '.join(dict.fromkeys(str(v).strip() for v in tags[key] if str(v).strip()))
    number = values.get('tracknumber', '').split('/')[0]
    if not number.isdigit() or int(number) < 1:
        raise ValueError('A valid track number is required')
    disc = values.get('discnumber', '').split('/')[0]
    values['disc_prefix'] = f'{int(disc):02d}.' if disc_relevant and disc.isdigit() else ''
    total = values.get('tracktotal') or values.get('totaltracks') or values.get('tracknumber', '').partition('/')[2]
    width = max(2, len(str(int(total)))) if total.isdigit() else 2
    disc_text = (f'{int(disc):02d}' if disc_padding else str(int(disc))) if disc.isdigit() else ''
    values.update(year=year, tracknumber=f'{int(number):0{width}d}', disc=f'Disc {disc_text}' if disc_relevant and disc_text else '')
    if disc.isdigit():
        values['discnumber'] = f'{int(disc):02d}'

    # Ensure album_artist and track_number aliases are populated
    album_artist_val = values.get('albumartist') or values.get('album_artist') or values.get('artist', '')
    values['albumartist'] = album_artist_val
    values['album_artist'] = album_artist_val
    values['tracknumber'] = values.get('tracknumber', '')
    values['track_number'] = values.get('tracknumber', '')
    values['album'] = values.get('album', '')
    values['album_title'] = values.get('album', '')
    values['title'] = values.get('title', '')
    values['track_title'] = values.get('title', '')
    parts = []
    for part in template.split('/'):
        if part == '{album} ({year})' and year and values.get('album', '').endswith(f'({year})'):
            part = '{album}'
        fields = [field for _, field, _, _ in string.Formatter().parse(part) if field]
        if part == '{disc}' and not values['disc']:
            continue
        if not year:
            part = part.replace(' ({year})', '').replace('({year})', '').replace('{year}', '')
        for field in fields:
            if field not in ('disc', 'disc_prefix', 'year') and not values.get(field):
                raise ValueError(f'Missing layout tag: {field}')
        rendered = part.format_map({k: safe_component(values.get(k, ''), final=False) if values.get(k) else '' for k in fields})
        parts.append(safe_component(rendered))
    parts[-1] += extension
    return Path(root).joinpath(*parts)


def normalize(audio, metadata=None):
    metadata = metadata or {}
    for key, values in metadata.items():
        if values:
            audio[key] = [str(v) for v in values]
    tags = {k: list(v) for k, v in audio.tags.items()}
    for kind in ('track', 'disc'):
        parts = str(tags.get(kind + 'number', [''])[0]).split('/')
        raw_total = tags.get(kind + 'total', tags.get('total' + kind + 's', [parts[1] if len(parts) > 1 else '']))[0]
        num = parts[0]
        if not num.isdecimal() or int(num) < 1:
            raise ValueError(f'Download is missing a valid {kind} number')
        if not str(raw_total).isdecimal() or int(raw_total) < int(num):
            raise ValueError(f'Download is missing a valid {kind} total')
        audio[kind + 'number'] = [f'{int(num):02d}']
        audio[kind + 'total'] = [f'{int(raw_total):02d}']
        if 'total' + kind + 's' in audio:
            audio['total' + kind + 's'] = [f'{int(raw_total):02d}']
    for key in ('date', 'originaldate', 'releasedate'):
        if key in audio:
            date_val = normalized_date(audio[key][0])
            if not date_val:
                raise ValueError(f'Download has an invalid {key}; review its metadata')
            audio[key] = [date_val if len(date_val) == 10 else date_val[:4]]
    # Key normalization
    raw_key = str(tags.get('initialkey', [''])[0]).strip()
    if raw_key in CAMELOT_MAP:
        audio['initialkey'] = [CAMELOT_MAP[raw_key]]
    for key in list(audio.keys()):
        if key.lower() in LYRIC_TAGS:
            del audio[key]
    for key in ('albumartist', 'artist'):
        if not audio.get(key):
            raise ValueError(f'Download is missing {key} credits')
    return {k: list(v) for k, v in audio.tags.items()}


def companion_cover(audio, album_root):
    pictures = getattr(audio, 'pictures', [])
    from .tag_io import MP4
    pictures = sorted((p for p in pictures if p.type == 3 or isinstance(audio, MP4)), key=lambda p: p.width * p.height, reverse=True)
    data = pictures[0].data if pictures else next(iter((audio.tags or {}).get('covr', [])), None)
    if data is None:
        return
    from PIL import Image
    with Image.open(io.BytesIO(data)) as image:
        if image.width * image.height > 40_000_000:
            raise ValueError('Cover image is too large')
        data = io.BytesIO()
        image.convert('RGB').save(data, format='JPEG', quality=95)
    target = Path(album_root) / 'cover.jpg'
    if not target.exists():
        with target.open('xb') as stream:
            stream.write(data.getvalue())


def download_path(stage, tags, template=None, extension='.flac'):
    date_val = tags.get('date', [''])[0]
    relevant = int(tags.get('disctotal', [1])[0]) > 1
    return layout_path(
        stage,
        tags,
        date_val[:4] if date_val[:4].isdecimal() else '',
        relevant,
        template=template or DOWNLOAD_LAYOUT,
        disc_padding=False,
        extension=extension,
    )


def normalize_m4a(audio, metadata, track_id, album_id):
    tags = normalize(audio, metadata)
    audio['tidal_track_id'] = [str(track_id)]
    audio['tidal_album_id'] = [str(album_id)]
    return tags, audio
