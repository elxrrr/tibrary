"""Read extended metadata in the isolated python-tidal runtime. No media endpoints."""
import contextlib
import json
from pathlib import Path
import sys
import time

_RESOURCES = Path(__file__).resolve().parents[1] / 'resources'
for _pkg in (_RESOURCES / 'tidaler', _RESOURCES / 'python-tidal'):
    if Path(sys.prefix).parent == _RESOURCES / 'tidaler' and _pkg.is_dir() and str(_pkg) not in sys.path:
        sys.path.insert(0, str(_pkg))

if __package__:
    from .download_bridge import emit,QuietOutput
else:
    from download_bridge import emit,QuietOutput


def read_metadata(session,ident,album_id,albums,pause=time.sleep):
    track=session.track(ident)
    artists = [a.name for a in track.artists] if getattr(track, 'artists', None) else []
    title = getattr(track, 'full_name', None) or getattr(track, 'name', '') or getattr(track, 'title', '')
    metadata=dict(id=str(track.id),isrc=getattr(track, 'isrc', None),bpm=getattr(track, 'bpm', None),
                  key=getattr(track, 'key', None),key_scale=getattr(track, 'key_scale', None),
                  title=title,artists=artists,copyright=getattr(track, 'copyright', None))
    if album_id:
        # A recording may appear on several editions. Never use its default album's UPC.
        if album_id not in albums:
            pause(.5)
            try:
                album=session.album(album_id)
                date=getattr(album, 'release_date', None)
                album_artists = [a.name for a in album.artists] if getattr(album, 'artists', None) else []
                barcode = getattr(album, 'upc', None) or getattr(album, 'universal_product_number', None) or ''
                albums[album_id]=dict(id=str(getattr(album, 'id', album_id)),
                                      title=getattr(album, 'name', '') or getattr(album, 'title', ''),
                                      album_artists=album_artists,
                                      date=date.date().isoformat() if date and hasattr(date, 'date') else '',
                                      barcode=barcode,
                                      copyright=getattr(album, 'copyright', '') or '',
                                      type=getattr(album, 'type', '') or '',
                                      disc_count=getattr(album, 'num_volumes', 1) or 1)
            except Exception:
                albums[album_id]=None
        if albums.get(album_id):
            metadata['release']=albums[album_id]
    return metadata


def main():
    from tidaler.config import Settings,Tidal
    tidal=Tidal(Settings())
    if not tidal.login_token():
        emit('unavailable',message='Connect the subscriber metadata account in Settings to look up extended metadata.');return
    session=tidal.session
    user=getattr(session,'user',None)
    market=(getattr(session,'country_code',None) or getattr(session,'countryCode',None)
            or getattr(user,'country_code',None) or getattr(user,'countryCode',None) or '')
    emit('ready',market=str(market).upper())
    albums={}
    for line in sys.stdin:
        request=json.loads(line);ident=str(request['id'])
        if not ident.isdecimal():raise ValueError('Invalid track ID')
        album_id=request.get('album_id')
        if album_id is not None and not str(album_id).isdecimal():raise ValueError('Invalid album ID')
        try:
            metadata=read_metadata(tidal.session,ident,album_id,albums)
            emit('metadata',track=metadata)
        except Exception as exc:
            status = getattr(getattr(exc, 'response', None), 'status_code', None)
            if not status and hasattr(exc, '__cause__'):
                status = getattr(getattr(exc.__cause__, 'response', None), 'status_code', None)
            if not status and ('ObjectNotFound' in type(exc).__name__ or 'NotFound' in type(exc).__name__):
                status = 404

            if status == 404:
                # Individual track or album not found in current catalogue/region.
                # Do NOT terminate the bridge or pause subsequent lookups!
                emit('track_unavailable', id=ident, album_id=album_id, message=f'Track {ident} not available online.')
                continue

            hint={401:'Reconnect the subscriber metadata account in Settings.',
                  403:'The subscriber metadata account was denied access.',
                  429:'TIDAL rate-limited the metadata account; retry later.'}.get(status,'Check the metadata connection in Settings.')
            emit('unavailable',message=f'Extended metadata lookup failed · track {ident} · album {album_id or "not requested"}'+(f' (HTTP {status})' if status else '')+f'. {hint} Remaining extended lookups paused for this job.');return


if __name__=='__main__':
    with contextlib.redirect_stdout(QuietOutput()),contextlib.redirect_stderr(QuietOutput()):
        try:main()
        except Exception:emit('unavailable',message='Extended metadata lookup could not start. Check the account connection in Settings.')
