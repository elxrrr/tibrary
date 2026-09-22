"""Shared catalogue cache for every desktop workflow; credentials are never cached here."""
import copy
import time
from .tidal import Tidal


class CachedTidal(Tidal):
    def __init__(self,*args,store,**kwargs):
        super().__init__(*args,**kwargs)
        self.store=store

    def _release_key(self,ident):return f'tag-review:{self.market}:{ident}'

    def _fresh(self,value,field,days):
        stamp=value.get(field,0)
        return bool(stamp and (days==0 or time.time()-stamp<days*86400))

    @staticmethod
    def _consistent(release,saved):
        # A newly fetched summary can invalidate a previously complete item list.
        expected=release.get('item_count',release.get('track_count'))
        actual=saved.get('track_count') if release.get('tracks_loaded') and 'item_count' not in release else saved.get('item_count',saved.get('track_count'))
        return expected is None or actual is None or int(expected)==int(actual)

    @staticmethod
    def _combine(release,saved):
        result=dict(copy.deepcopy(release),**copy.deepcopy(saved))
        if (release.get('link_checked_at') or 0)>(saved.get('link_checked_at') or 0):
            result.update(available=release.get('available'),link_checked_at=release['link_checked_at'])
        return result

    def release_details(self,release,force=False):
        self.check()
        saved=self.store.preferences(self._release_key(release['id']))
        days=self.store.preferences('release_links',{'max_age_days':30}).get('max_age_days',30)
        if not force and self._consistent(release,saved) and saved.get('tracks_loaded') and self._fresh(saved,'details_checked_at',days):
            return self._combine(release,saved)
        if not force and self._consistent(release,saved) and saved.get('tracks_loaded') and self._fresh(saved,'tag_checked_at',days):
            return self._combine(release,saved)
        result=super().release_details(release)
        result['details_checked_at']=time.time()
        # Don't refresh an older tag/credit timestamp when only the track list changed.
        merged=dict(saved,**result)
        if saved.get('tracks') and [(t.get('id'),t.get('track_number'),t.get('disc_number')) for t in saved['tracks']]!=[(t.get('id'),t.get('track_number'),t.get('disc_number')) for t in result['tracks']]:
            merged.pop('tag_credits_checked',None);merged.pop('tag_checked_at',None);merged.pop('metadata_schema',None)
        self.store.save_preferences(self._release_key(release['id']),merged)
        return copy.deepcopy(merged)

    def album_tag_details(self,release):
        self.check()
        saved=self.store.preferences(self._release_key(release['id']))
        if self._consistent(release,saved) and saved.get('tracks_loaded') and saved.get('tag_credits_checked') and saved.get('metadata_schema')==3 and self._fresh(saved,'tag_checked_at',1):
            return self._combine(release,saved)
        result=super().album_tag_details(release)
        result.update(tag_checked_at=time.time(),metadata_schema=3)
        self.store.save_preferences(self._release_key(release['id']),result)
        return copy.deepcopy(result)

    def track_tag_details(self,track):
        self.check()
        key=f'tag-track:{self.market}:{track["id"]}'
        saved=self.store.preferences(key)
        if self._fresh(saved,'checked_at',1) and isinstance(saved.get('track'),dict):
            return copy.deepcopy(saved['track'])
        result=super().track_tag_details(track)
        self.store.save_preferences(key,dict(checked_at=time.time(),track=result))
        return copy.deepcopy(result)
