"""Tag-based folder layouts. No filesystem writes or inferred artist names."""
import re
import string
import sys
import unicodedata
from pathlib import Path

LEGACY_LAYOUT = '{albumartist}/{album} ({year})/{disc}/{tracknumber} - {title}'
DEFAULT_LAYOUT = '{albumartist}/{album} ({year})/{disc}/{disc_prefix}{tracknumber} - {title}'


def safe_component(value, final=True):
    """One portable Windows/macOS component; tags themselves are untouched."""
    raw=unicodedata.normalize('NFC', str(value))
    def separator(match):
        left=raw[:match.start()].rstrip();right=raw[match.end():].lstrip()
        return ' - ' if left and right and left[-1] not in '([{' and right[0] not in ')]}' else ''
    cleaned=re.sub(r'\s*[/\\:|]+\s*',separator,raw)
    cleaned=re.sub(r'[\x00-\x1f\x7f*?"<>]', '', cleaned).strip()
    if cleaned!=raw:cleaned=re.sub(' {2,}',' ',cleaned)
    if final:cleaned=cleaned.rstrip(' .')
    if cleaned in ('', '.', '..'):raise ValueError('Tags produce an empty or unsafe filename component')
    if final and re.match(r'^(CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])(?:\.|$)',cleaned,re.I):cleaned='_'+cleaned
    return cleaned


def name_key(value):
    return unicodedata.normalize('NFC', str(value))


def path_tag_notes(tags, layout=None):
    """Explain path-only sanitisation without implying the current name is illegal."""
    template=(layout or {}).get('template',DEFAULT_LAYOUT)
    fields={field for _,field,_,_ in string.Formatter().parse(template) if field}
    notes=['Organisation follows the saved tags, even when the existing path is already valid.']
    for key in sorted(fields):
        values=tags.get(key,[])
        if not values:continue
        raw=', '.join(dict.fromkeys(values)) if key in ('albumartist','artist') else str(values[0])
        try:clean=safe_component(raw)
        except ValueError:continue  # The planner already explains unsafe/empty components.
        if clean!=raw:
            notes.append(f'Path text from {key}: “{raw}” → “{clean}” · portable path punctuation; tag unchanged.')
    return notes


def validate_layout(template):
    if not template or template.startswith('/') or '\\' in template:
        raise ValueError('Use a relative layout with / between folders')
    for part in template.split('/'):
        if not part or part in ('.','..'): raise ValueError('Layout contains an empty or unsafe folder')
        for _,field,format_spec,conversion in string.Formatter().parse(part):
            if field is not None and (not re.fullmatch(r'[a-z][a-z0-9_]*', field) or format_spec or conversion):
                raise ValueError('Use plain tag names such as {albumartist}, without format expressions')
    if template.lower().endswith(('.flac','.mp3','.m4a')):raise ValueError('Leave off the file extension; it is added automatically')
    if '{title}' not in template.split('/')[-1]: raise ValueError('The filename must include {title}')
    return template


def layout_path(root, tags, year='', disc_relevant=False, current_path=None, template=DEFAULT_LAYOUT, extension='.flac', disc_padding=True):
    validate_layout(template)
    values = {k.lower(): str(v[0]).strip() for k,v in tags.items() if v}
    for key in ('albumartist','artist'):
        if tags.get(key):values[key]=', '.join(dict.fromkeys(str(v).strip() for v in tags[key] if str(v).strip()))
    number = values.get('tracknumber','').split('/')[0]
    if not number.isdigit() or int(number)<1: raise ValueError('A valid track number is required')
    disc = values.get('discnumber','').split('/')[0]
    values['disc_prefix']=f'{int(disc):02d}.' if disc_relevant and disc.isdigit() else ''
    total = values.get('tracktotal') or values.get('totaltracks') or values.get('tracknumber','').partition('/')[2]
    width = max(2, len(str(int(total)))) if total.isdigit() else 2
    disc_text = (f'{int(disc):02d}' if disc_padding else str(int(disc))) if disc.isdigit() else ''
    values.update(year=year,tracknumber=f'{int(number):0{width}d}',disc=f'Disc {disc_text}' if disc_relevant and disc_text else '')
    if disc.isdigit():values['discnumber']=f'{int(disc):02d}'
    parts=[]
    for part in template.split('/'):
        if part=='{album} ({year})' and year and values.get('album','').endswith(f'({year})'):part='{album}'
        fields=[field for _,field,_,_ in string.Formatter().parse(part) if field]
        if part=='{disc}' and not values['disc']: continue
        if not year:part=part.replace(' ({year})','').replace('({year})','').replace('{year}','')
        for field in fields:
            if field not in ('disc','disc_prefix','year') and not values.get(field):raise ValueError(f'Missing layout tag: {field}')
        # Sanitize each tag BEFORE formatting; a slash inside a title must never
        # turn into an extra directory in the layout.
        rendered=part.format_map({k:safe_component(values.get(k,''),final=False) if values.get(k) else '' for k in fields})
        parts.append(safe_component(rendered))
    parts[-1]+=extension
    if any(len(name_key(p).encode('utf-8'))>255 for p in parts):
        raise ValueError('A path component is too long; shorten the tag or choose another layout')
    if current_path:
        current=Path(current_path).relative_to(root).parts
        # Case and canonical Unicode spelling are not content differences. Keep
        # their on-disk spelling, but never keep obsolete punctuation substitutions.
        for i,part in enumerate(parts):
            old = current[-1] if i==len(parts)-1 else (current[i] if i<len(current)-1 else '')
            if name_key(old).casefold()==name_key(part).casefold():parts[i]=old
    return Path(root).joinpath(*parts)
