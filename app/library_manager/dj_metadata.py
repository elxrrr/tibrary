"""Lazy, paced, cached metadata-only access to the isolated python-tidal session."""
import json
import os
from pathlib import Path
import selectors
import subprocess
import time
from .backends import active_runtime
from .tidal import RequestPacer


class DJMetadata:
    def __init__(self,store,cancel=lambda:False,progress=lambda s:None,pacer=None,process_factory=subprocess.Popen,market='GB'):
        self.store,self.cancel,self.progress=store,cancel,progress
        self.pacer=pacer or RequestPacer();self.factory=process_factory
        self.market=str(market or 'GB').upper();self.session_market=''
        self.process=None;self.selector=None;self.buffer=b'';self.disabled=False

    def __enter__(self):return self
    def __exit__(self,*args):self.close()

    def close(self):
        if self.selector:self.selector.close();self.selector=None
        if self.process:
            self.process.stdin.close()
            if self.process.poll() is None:
                self.process.terminate()
                try:self.process.wait(timeout=2)
                except subprocess.TimeoutExpired:self.process.kill();self.process.wait(timeout=2)
            self.process.stdout.close();self.process=None

    def receive(self):
        until=time.monotonic()+20
        while time.monotonic()<until and not self.cancel():
            if b'\n' in self.buffer:
                line,self.buffer=self.buffer.split(b'\n',1)
                return json.loads(line)
            events=self.selector.select(.1)
            if events:
                data=os.read(self.process.stdout.fileno(),65536)
                if not data:raise ValueError('Metadata session ended')
                self.buffer+=data
                if len(self.buffer)>65536:raise ValueError('Invalid metadata response')
            elif self.process.poll() is not None:raise ValueError('Metadata session ended')
        raise TimeoutError('Metadata lookup cancelled or timed out')

    def lookup(self,track,album_id=None,force=False):
        ident=str(track['id']);album_id=str(album_id) if album_id is not None else None
        key=f'extended-metadata:v3:{self.market}:{ident}:{album_id or ""}'
        if not ident.isdecimal() or (album_id is not None and not album_id.isdecimal()) or self.cancel():return {}
        cached=self.store.preferences(key)
        if not cached and self.market=='GB':
            legacy=self.store.preferences(f'extended-metadata:v2:{ident}:{album_id or ""}')
            if legacy:
                cached=legacy
                self.store.save_preferences(key,dict(legacy,source_market='legacy-GB'))
        if not force and time.time()-cached.get('checked_at',0)<86400:
            if cached.get('unavailable'):
                return {}
            try:return self.verified(track,cached.get('track',{}),album_id)
            except ValueError:pass
        if self.disabled:return {}
        try:
            if not self.process:
                config=self.store.path.parent/'downloader-session'
                if not config.is_dir():raise ValueError('No saved metadata account')
                env=dict(os.environ,PYTHONUNBUFFERED='1',XDG_CONFIG_HOME=str(config))
                self.process=self.factory([str(active_runtime()),'-u',str(Path(__file__).with_name('metadata_bridge.py'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env=env,start_new_session=True)
                self.selector=selectors.DefaultSelector();self.selector.register(self.process.stdout,selectors.EVENT_READ)
                message=self.receive()
                if message.get('event')!='ready':raise ValueError('Metadata account unavailable')
                self.session_market=str(message.get('market') or '').upper()
                if self.session_market and self.session_market!=self.market:
                    self.progress(f'Extended metadata account uses {self.session_market}; release availability continues to be verified for {self.market}.')
                self.progress('Reading extended track and release metadata · metadata only')
            self.pacer.wait(self.cancel)
            self.process.stdin.write((json.dumps({'id':ident,'album_id':album_id})+'\n').encode());self.process.stdin.flush()
            message=self.receive()
            if message.get('event')=='track_unavailable':
                self.store.save_preferences(key,dict(checked_at=time.time(),unavailable=True))
                return {}
            if message.get('event')!='metadata':
                self.progress(message.get('message','Extended metadata lookup paused.'));raise ValueError('Metadata unavailable')
            try:
                supplied=self.verified(track,message['track'],album_id)
            except ValueError:
                self.store.save_preferences(key,dict(checked_at=time.time(),unavailable=True))
                return {}
            self.store.save_preferences(key,dict(checked_at=time.time(),track=dict(supplied,id=ident),
                                                 source_market=self.session_market or self.market))
            return supplied
        except (OSError,ValueError,KeyError,TimeoutError):
            self.disabled=True;self.close()
            if not self.cancel():self.progress('Extended metadata is unavailable for this job. Check the subscriber account in Settings. Existing tags retained.')
            return {}

    @staticmethod
    def verified(expected,returned,album_id=None):
        if str(returned.get('id'))!=str(expected['id']):raise ValueError('Metadata recording ID differs')
        if expected.get('isrc') and returned.get('isrc') and expected['isrc'].replace('-','').upper()!=returned['isrc'].replace('-','').upper():raise ValueError('Metadata ISRC differs')
        result={k:returned.get(k) for k in ('isrc','bpm','key','key_scale','initialkey','tkey','musical_key','title','artists','copyright')}
        release=returned.get('release')
        if release is not None:
            if album_id is None or str(release.get('id'))!=str(album_id):raise ValueError('Metadata release ID differs')
            result['release']={k:release.get(k) for k in ('id','title','album_artists','date','barcode','copyright','type','disc_count')}
        return result
