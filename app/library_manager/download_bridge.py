"""Isolated Python 3.13 tidaler adapter; communicates over JSON lines."""
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import sys
import tempfile
import threading
import time

_RESOURCES = Path(__file__).resolve().parents[1] / 'resources'
for _pkg in (_RESOURCES / 'tidaler',):
    if Path(sys.prefix).parent == _RESOURCES / 'tidaler' and _pkg.is_dir() and str(_pkg) not in sys.path:
        sys.path.insert(0, str(_pkg))


def emit(event, **data):
    sys.__stdout__.write(json.dumps(dict(event=event, **data))+'\n');sys.__stdout__.flush()


class Log:
    def info(self, message, *args, **kwargs):
        text=re.sub(r'https?://\S+', '[TIDAL URL]',str(message))
        emit('log',message=text[:1000])
    warning=error=exception=info
    def debug(self,*args,**kwargs):pass


class QuietOutput:
    def write(self,text):return len(text)
    def flush(self):pass


def publish_files(stage, output):
    """Publish new files exclusively; never replace an existing library file."""
    stage = Path(stage).resolve()
    output = Path(output).resolve()
    published=[]
    for source in stage.rglob('*'):
        if not source.is_file() or source.is_symlink():continue
        target=output/source.relative_to(stage)
        if not target.resolve().is_relative_to(output):raise ValueError('Unsafe download destination')
        target.parent.mkdir(parents=True,exist_ok=True)
        try:os.link(source,target)
        except FileExistsError:
            if target.is_symlink() or not target.is_file():raise ValueError('Destination collision')
            if source.name=='cover.jpg':
                published.append(str(target));continue  # Existing artwork belongs to the user.
            with open(source,'rb') as a,open(target,'rb') as b:
                if hashlib.file_digest(a,'sha256').digest()!=hashlib.file_digest(b,'sha256').digest():
                    raise ValueError(f'Destination exists with different contents: {target.name}')
        published.append(str(target))
    return published


def organise_download(path,stage,layout,track_id,album_id,metadata=None,destination=None):
    # Imported here so the small subprocess can use the app's path policy without Qt.
    sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
    from library_manager.tag_io import open_audio
    audio=open_audio(path)
    if audio is None or not audio.tags:raise ValueError('Downloaded tags are unavailable; cannot organise this file safely')
    from library_manager.download_metadata import normalize, download_path, companion_cover
    if path.suffix.casefold() in ('.m4a','.mp4'):
        from library_manager.download_metadata import normalize_m4a
        tags,audio=normalize_m4a(audio,metadata,track_id,album_id)
    else:tags=normalize(audio, metadata)
    target=download_path(stage,tags,layout.get('template'),path.suffix)
    if destination:
        relative=Path(destination['album_relative'])
        if relative.is_absolute() or '..' in relative.parts:raise ValueError('Unsafe existing album destination')
        album_root=stage/relative
        disc_dir=destination.get('disc_dirs',{}).get(tags['discnumber'][0])
        if disc_dir is None:disc_dir=f"Disc {int(tags['discnumber'][0])}" if int(tags['disctotal'][0])>1 else ''
        disc_path=Path(disc_dir)
        if disc_path.is_absolute() or '..' in disc_path.parts:raise ValueError('Unsafe existing disc destination')
        # Reuse the album/disc folder; new filenames still follow the portable policy.
        filename=download_path(stage,tags,extension=path.suffix).name
        target=album_root/disc_path/filename
    if not target.resolve().is_relative_to(Path(stage).resolve()):raise ValueError('Unsafe organised download path')
    if path.suffix.casefold()=='.flac':
        audio['tidal_track_id']=[track_id];audio['tidal_album_id']=[album_id]
    audio.save()
    if target!=path:
        target.parent.mkdir(parents=True,exist_ok=True);os.link(path,target);path.unlink()
    # Write companion art in the album root, above a disc directory when present.
    if not destination:album_root = target.parent.parent if int(tags['disctotal'][0])>1 and target.parent.name.startswith('Disc ') else target.parent
    companion_cover(audio,album_root)
    return target


def load_album_tracks(album, cancelled=lambda:False, pause=time.sleep):
    """Bounded, deduplicated pagination, also for large/multi-disc releases."""
    expected=getattr(album,'num_tracks',None)
    expected=int(expected) if expected and int(expected)>0 else None
    tracks=[];seen=set()
    while len(tracks)<10000:
        if cancelled():raise ValueError('Download cancelled')
        page=list(album.tracks(limit=100,offset=len(tracks)))
        if any(str(t.id) in seen for t in page):raise ValueError('Repeated album page; download stopped')
        if len({str(t.id) for t in page})!=len(page):raise ValueError('Duplicate album track IDs; review release')
        tracks.extend(page);seen.update(str(t.id) for t in page)
        if (expected and len(tracks)>=expected) or len(page)<100:break
        pause(.4)
    if not tracks or (expected and len(tracks)!=expected) or len(tracks)>=10000:
        raise ValueError('Incomplete album track list; review this release again')
    return tracks


def main():
    request=json.loads(sys.stdin.readline())
    output=Path(request['output']).expanduser().resolve()
    if not request.get('check_only'):output.mkdir(parents=True,exist_ok=True)
    os.umask(0o077)
    from tidaler.config import Settings,Tidal
    from tidaler.download import Download
    sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
    from library_manager.download_tags import install_download_adapter
    install_download_adapter()
    from tidaler.constants import CoverDimensions
    from tidalapi import Quality
    from rich.progress import Progress
    preferences=request.get('settings',{})
    q_map = {
        'Hi-res lossless': getattr(Quality, 'hi_res_lossless', Quality.high_lossless),
        'Lossless': getattr(Quality, 'high_lossless', Quality.high_lossless),
        'High': getattr(Quality, 'high', Quality.high_lossless),
        'Low': getattr(Quality, 'low', Quality.high_lossless),
    }
    quality_name = {'LOSSLESS':'Lossless','HI_RES_LOSSLESS':'Hi-res lossless','HIGH':'High','LOW':'Low'}.get(preferences.get('quality'), preferences.get('quality','Lossless'))
    quality = q_map.get(quality_name, Quality.high_lossless)
    if quality_name=='High' and int(preferences.get('aac_bitrate_cap',320))<=96:
        quality=q_map['Low']
    settings=Settings();settings.data.quality_audio=quality
    # A packaged app cannot depend on a Homebrew executable in the user's PATH.
    try:
        import imageio_ffmpeg
        settings.data.path_binary_ffmpeg=os.environ.get('TIBRARY_FFMPEG') or imageio_ffmpeg.get_ffmpeg_exe()
    except ImportError:
        if os.environ.get('TIBRARY_FFMPEG'):settings.data.path_binary_ffmpeg=os.environ['TIBRARY_FFMPEG']
    settings.data.lyrics_embed=False
    settings.data.metadata_replay_gain=bool(preferences.get('replaygain',True))
    settings.data.cover_album_file=False;settings.data.lyrics_file=False
    cover_size_str = str(preferences.get('cover_size', '1280')).lower()
    cover_dim_map = {
        '1280': CoverDimensions.Px1280,
        '640': CoverDimensions.Px640,
        '320': CoverDimensions.Px320,
        'origin': CoverDimensions.PxORIGIN,
        'source': CoverDimensions.PxORIGIN,
    }
    dim = cover_dim_map.get(cover_size_str, CoverDimensions.Px1280)
    settings.data.metadata_cover_embed=True;settings.data.metadata_cover_dimension=dim
    settings.data.skip_existing=True;settings.data.use_primary_album_artist=True
    settings.data.symlink_to_track=False;settings.data.album_track_num_pad_min=2
    settings.data.video_download=False;settings.data.download_dolby_atmos=False
    settings.data.extract_flac=True;settings.data.video_convert_mp4=False
    settings.data.downloads_simultaneous_per_track_max=max(1,min(20,int(preferences.get('segment_concurrency',preferences.get('segments',2)))))
    settings.data.downloads_concurrent_max=max(1,min(4,int(preferences.get('download_concurrency',3))))
    settings.data.download_delay=bool(preferences.get('download_delay',True))
    settings.data.download_delay_sec_min=max(0,min(30,float(preferences.get('download_delay_min_sec',3))))
    settings.data.download_delay_sec_max=max(settings.data.download_delay_sec_min,min(30,float(preferences.get('download_delay_max_sec',5))))
    settings.data.api_rate_limit_batch_size=max(1,min(100,int(preferences.get('api_batch_size',20))))
    settings.data.api_rate_limit_delay_sec=max(0,min(60,float(preferences.get('api_batch_delay_sec',3))))
    tidal=Tidal(settings)
    abort=threading.Event();running=threading.Event();running.set()
    signal.signal(signal.SIGTERM,lambda *args:abort.set())
    logged_in = tidal.login_token()
    if request.get('check_only'):
        emit('connection',connected=bool(logged_in))
        return 0
    if not logged_in:
        emit('auth',url=tidal.session.pkce_login_url())
        reply=json.loads(sys.stdin.readline())
        if abort.is_set() or not reply.get('redirect'):return 2
        # Authentication URLs and token responses are never forwarded to logs.
        try:
            token=tidal.session.pkce_get_auth_token(reply['redirect'])
            tidal.session.process_auth_token(token,is_pkce_token=True)
            if not tidal.login_finalize():raise ValueError('Login unsuccessful')
        except Exception:
            emit('error',message='Download sign-in failed. Reconnect and try again.');return 1
    emit('log',message='Tidaler account connected · lossless audio · sequential track downloads')
    for queued in request['items']:
        if abort.is_set():break
        ident=str(queued['id']);release=queued['release']
        album_artist_name = release.get('artist') or ''
        emit('log',message=f"Downloading {album_artist_name or 'Release'} — {release.get('title', '')}")
        selected=release.get('selected_tracks')
        destination=release.get('existing_destination')
        item_output=output
        if destination:
            requested=Path(destination['root']).expanduser()
            if not requested.is_dir() or requested.resolve()!=requested:raise ValueError('Existing library is offline or follows a symbolic link')
            item_output=requested
            album_path=item_output/str(destination['album_relative'])
            if not album_path.resolve().is_relative_to(item_output) or not album_path.is_dir():raise ValueError('Existing album folder is unavailable')
        item_quality=q_map.get(release.get('replacement_audit',{}).get('quality'),quality)
        try:
            album=tidal.session.album(ident)
            album_tracks=load_album_tracks(album,abort.is_set,abort.wait)
            tracks=[t for t in album_tracks if str(t.id) in {str(s['id']) for s in selected}] if selected is not None else album_tracks
            if selected is not None and len(tracks)!=len(selected):raise ValueError('Selected tracks no longer belong to this release')
            if not tracks:raise ValueError('No audio tracks returned')
            if selected is None and release.get('track_count') is not None and len(tracks)!=release['track_count'] and not (not release.get('tracks_loaded') and int(getattr(album,'num_videos',0) or 0)>0 and len(tracks)+int(album.num_videos)==int(release['track_count'])):
                raise ValueError('Incomplete album track list; review this release again')
            if not album_artist_name:
                if getattr(album, 'artist', None) and getattr(album.artist, 'name', None):
                    album_artist_name = album.artist.name
                elif getattr(album, 'artists', None):
                    names = [a.name for a in album.artists if getattr(a, 'name', None)]
                    if names:
                        album_artist_name = ', '.join(names)
            from library_manager.download_tags import DownloadMetadata
            DownloadMetadata.current_album_artist = album_artist_name or None
            with tempfile.TemporaryDirectory(prefix='.tidaler-work-',dir=item_output) as temp:
                stage=Path(temp).resolve()
                progress=Progress(disable=True);overall=Progress(disable=True)
                dl=Download(tidal_obj=tidal,path_base=str(stage),fn_logger=Log(),skip_existing=True,
                            progress=progress,progress_overall=overall,event_abort=abort,event_run=running)
                for index,track in enumerate(tracks,1):
                    if abort.is_set():break
                    if not getattr(track,'available',False):raise ValueError('A selected track is unavailable')
                    emit('log',message=f'Track {index}/{len(tracks)} · {track.name}')
                    stop_reporting=threading.Event()
                    def report():
                        previous=-1
                        while not stop_reporting.wait(2):
                            if progress.tasks:
                                percent=int(progress.tasks[-1].percentage)
                                if percent!=previous:
                                    emit('log',message=f'Download progress · {percent}%');previous=percent
                    reporter=threading.Thread(target=report,daemon=True);reporter.start()
                    template=f'.source-{track.id}'
                    try:
                        success,path=dl.item(media=track,file_template=template,video_download=False,download_delay=settings.data.download_delay,quality_audio=item_quality)
                    finally:stop_reporting.set();reporter.join(timeout=3)
                    if abort.is_set():break
                    if not success or not path or not Path(path).is_file():raise ValueError('Tidaler did not complete this track')
                    if not Path(path).resolve().is_relative_to(stage):raise ValueError('Unexpected download path')
                    track_artist_names = [a.name for a in track.artists if getattr(a, 'name', None)] if getattr(track, 'artists', None) else ([album_artist_name] if album_artist_name else [])
                    album_artist_list = [album_artist_name] if album_artist_name else track_artist_names
                    disc=int(track.volume_num)
                    metadata=dict(artist=track_artist_names,albumartist=album_artist_list,
                                  album=[album.name+(f' ({album.version})' if getattr(album,'version',None) and str(album.version).casefold() not in album.name.casefold() else '')],title=[getattr(track,'full_name',None) or track.name],tracknumber=[track.track_num],
                                  tracktotal=[sum(int(t.volume_num)==disc for t in album_tracks)],
                                  discnumber=[disc],disctotal=[album.num_volumes if album.num_volumes and album.num_volumes>0 else max(int(t.volume_num) for t in album_tracks)])
                    album_date=getattr(album,'available_release_date',None)
                    if album_date:metadata['date']=[album_date.date().isoformat()]
                    metadata.update(release.get('preserve_local_dj',{}).get(str(track.id),{}))
                    organise_download(Path(path),stage,request.get('layout',{}),str(track.id),str(album.id),metadata,destination)
                if abort.is_set():break
                published=publish_files(stage,item_output)
            emit('completed',id=ident,files=published)
        except Exception as exc:
            Log().error(f'Download incomplete · {type(exc).__name__}: {exc}')
            emit('failed',id=ident)
            # Stop rather than retrying a denied/rate-limited session repeatedly.
            return 1
    emit('finished',cancelled=abort.is_set())
    return 2 if abort.is_set() else 0


if __name__=='__main__':
    with contextlib.redirect_stdout(QuietOutput()),contextlib.redirect_stderr(QuietOutput()):
        try:code=main()
        except Exception as exc:
            emit('error',message=f'Downloader stopped: {type(exc).__name__}. Check connection and download sign-in.');code=1
    raise SystemExit(code)
