"""Verified front covers at 1280 px; never upscale a lower-resolution image."""
import hashlib
from pathlib import Path
from urllib.parse import urlparse
from urllib.request import Request,build_opener
from urllib.error import URLError
from .tidal import NoRedirect,CatalogueError

SIZE=1280


def cover_dimensions(data):
    from PIL import Image
    from io import BytesIO
    if len(data)>10*1024*1024:return None
    try:
        with Image.open(BytesIO(data)) as image:return image.size
    except (OSError,ValueError,Image.DecompressionBombError):return None


def normalise_cover(data):
    from PIL import Image
    from io import BytesIO
    if len(data)>10*1024*1024:raise ValueError('Cover image exceeds the size limit')
    try:
        with Image.open(BytesIO(data)) as image:
            width,height=image.size
            if width!=height or not SIZE<=width<=4096:
                raise ValueError('No square cover with at least 1280 × 1280 pixels; existing artwork retained')
            image.load()
            if width==SIZE and image.format=='JPEG':return data
            output=BytesIO()
            image.convert('RGB').resize((SIZE,SIZE),Image.Resampling.LANCZOS).save(output,'JPEG',quality=95)
            return output.getvalue()
    except OSError as exc:raise ValueError('Cover image could not be decoded') from exc


def prepare_cover(release,store,cancel=lambda:False,transport=None):
    choices=[]
    for item in release.get('cover_files',[]):
        meta=item.get('meta') or {};width=meta.get('width',0);height=meta.get('height',0)
        if not isinstance(width,(int,float)) or not isinstance(height,(int,float)):continue
        if width==height and SIZE<=width<=4096:choices.append(item)
    if not choices:return None
    item=min(choices,key=lambda i:i['meta']['width'])
    url=item.get('href','');parsed=urlparse(url)
    if parsed.scheme!='https' or parsed.hostname not in ('resources.tidal.com','images.tidal.com') or parsed.username or parsed.password or parsed.port not in (None,443):
        raise CatalogueError('Cover URL was not a supported TIDAL image host; existing artwork retained')
    cache=store.path.parent/'artwork-cache';cache.mkdir(parents=True,exist_ok=True)
    path=cache/(hashlib.sha256(url.encode()).hexdigest()+'.jpg')
    if cancel():return None
    try:
        if path.exists():data=normalise_cover(path.read_bytes())
        else:
            with (transport or build_opener(NoRedirect()).open)(Request(url),timeout=20) as response:data=response.read(10*1024*1024+1)
            data=normalise_cover(data)
            if cancel():return None
            temporary=path.with_suffix('.tmp');temporary.write_bytes(data);temporary.replace(path)
    except (URLError,OSError,ValueError) as exc:raise CatalogueError('Cover could not be upgraded: '+str(exc)) from None
    return dict(path=str(path),sha256=hashlib.sha256(data).hexdigest(),width=SIZE,height=SIZE)


def checked_picture(proposal):
    from .tag_io import Picture
    data=Path(proposal['path']).read_bytes()
    if hashlib.sha256(data).hexdigest()!=proposal['sha256']:raise ValueError('Prepared artwork changed; check covers again')
    picture=Picture();picture.type=3;picture.mime='image/jpeg';picture.desc='Front cover'
    picture.width=picture.height=SIZE;picture.depth=24;picture.data=data
    return picture


def prepare_existing_cover(row,store):
    from .tag_io import FLAC
    from .maintenance import fingerprint
    size=row.get('cover_size')
    if not size or size[0]!=size[1] or not SIZE<size[0]<=4096:return None
    if fingerprint(Path(row['path']))!=tuple(row['stamp']):raise ValueError('File changed since inspection; inspect again')
    picture=next((p for p in FLAC(row['path']).pictures if p.type==3),None)
    if picture is None:return None
    data=normalise_cover(picture.data);digest=hashlib.sha256(data).hexdigest()
    path=store.path.parent/'artwork-cache'/(digest+'.jpg');path.parent.mkdir(parents=True,exist_ok=True)
    if not path.exists():path.write_bytes(data)
    return dict(path=str(path),sha256=digest,width=SIZE,height=SIZE)
