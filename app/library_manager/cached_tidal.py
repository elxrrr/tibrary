"""Shared catalogue cache for every desktop workflow; credentials are never cached here."""

import copy
import time
import hashlib
import json
from .tidal import Tidal


class CachedTidal(Tidal):
    def __init__(self, *args, store, **kwargs):
        super().__init__(*args, **kwargs)
        self.store = store

    def _lookup(self, kind, identity, fetch, *, force=False, days=1):
        """Cache complete successes only; exceptions/cancellation never replace data.

        Keys include market and operation; short-lived empty results prevent repeated
        dead-end searches without hiding newly added catalogue entries indefinitely.
        """
        self.check()
        digest = hashlib.sha256(
            json.dumps(identity, sort_keys=True).encode()
        ).hexdigest()
        key = f"catalogue-lookup:v1:{self.market}:{kind}:{digest}"
        saved = self.store.preferences(key)
        age = days if saved.get("result") else min(days, 1 / 24)
        if not force and "result" in saved and self._fresh(saved, "checked_at", age):
            self.progress(f"Using saved {kind} · {identity} · {self.market}")
            return copy.deepcopy(saved["result"])
        self.progress(
            f"Checking online {kind} · {identity} · "
            + (
                "requested refresh"
                if force
                else "saved data missing or due for refresh"
            )
        )
        result = fetch()
        self.check()
        self.store.save_preferences(key, dict(checked_at=time.time(), result=result))
        return copy.deepcopy(result)

    def search(self, name):
        return self._lookup(
            "artist search", name, lambda: super(CachedTidal, self).search(name)
        )

    def search_albums(self, query):
        return self._lookup(
            "release search",
            query,
            lambda: super(CachedTidal, self).search_albums(query),
        )

    def search_tracks(self, query):
        return self._lookup(
            "track search", query, lambda: super(CachedTidal, self).search_tracks(query)
        )

    def recording_releases(self, isrc=None, track_id=None):
        return self._lookup(
            "recording editions",
            [isrc, track_id],
            lambda: super(CachedTidal, self).recording_releases(isrc, track_id),
        )

    def artist(self, ident, progress=lambda s: None, detailed=False, force=False):
        result = self._lookup(
            "artist releases",
            str(ident),
            lambda: super(CachedTidal, self).artist(ident, progress, detailed=False),
            force=force,
        )
        if detailed:
            result["releases"] = [self.release_details(r) for r in result["releases"]]
        return result

    def release_availability(self, release, force=False):
        # Reuse observations across workflows without overwriting richer track data.
        saved = self.store.preferences(self._release_key(release["id"]))
        if (
            not force
            and saved.get("available") is not None
            and self._fresh(saved, "link_checked_at", 1)
        ):
            return dict(
                release,
                available=saved["available"],
                link_checked_at=saved["link_checked_at"],
            )
        result = super().release_availability(release, force=force)
        self.check()
        if result.get("available") is not None:
            observed = result.get("link_checked_at") or time.time()
            self.store.save_preferences(
                self._release_key(release["id"]),
                {**release, **saved, "available": result["available"], "link_checked_at": observed},
            )
        return result

    def _release_key(self, ident):
        return f"tag-review:{self.market}:{ident}"

    def _fresh(self, value, field, days):
        stamp = value.get(field, 0)
        return bool(stamp and (days == 0 or time.time() - stamp < days * 86400))

    @staticmethod
    def _consistent(release, saved):
        # A newly fetched summary can invalidate a previously complete item list.
        expected = release.get("item_count", release.get("track_count"))
        actual = (
            saved.get("track_count")
            if release.get("tracks_loaded") and "item_count" not in release
            else saved.get("item_count", saved.get("track_count"))
        )
        return expected is None or actual is None or int(expected) == int(actual)

    @staticmethod
    def _combine(release, saved):
        result = dict(copy.deepcopy(release), **copy.deepcopy(saved))
        if (release.get("link_checked_at") or 0) > (saved.get("link_checked_at") or 0):
            result.update(
                available=release.get("available"),
                link_checked_at=release["link_checked_at"],
            )
        return result

    def release_details(self, release, force=False):
        self.check()
        saved = self.store.preferences(self._release_key(release["id"]))
        days = self.store.preferences("release_links", {"max_age_days": 30}).get(
            "max_age_days", 30
        )
        if (
            not force
            and self._consistent(release, saved)
            and saved.get("tracks_loaded")
            and self._fresh(saved, "details_checked_at", days)
        ):
            self.progress(
                f"Using saved release details · {release.get('title') or release['id']}"
            )
            return self._combine(release, saved)
        if (
            not force
            and self._consistent(release, saved)
            and saved.get("tracks_loaded")
            and self._fresh(saved, "tag_checked_at", days)
        ):
            self.progress(
                f"Using saved release details · {release.get('title') or release['id']}"
            )
            return self._combine(release, saved)
        self.progress(
            f"Loading release details · {release.get('title') or release['id']} · missing, changed or due for refresh"
        )
        result = super().release_details(release)
        self.check()
        result["details_checked_at"] = time.time()
        # Don't refresh an older tag/credit timestamp when only the track list changed.
        merged = dict(saved, **result)
        if saved.get("tracks") and [
            (t.get("id"), t.get("track_number"), t.get("disc_number"))
            for t in saved["tracks"]
        ] != [
            (t.get("id"), t.get("track_number"), t.get("disc_number"))
            for t in result["tracks"]
        ]:
            merged.pop("tag_credits_checked", None)
            merged.pop("tag_checked_at", None)
            merged.pop("metadata_schema", None)
        self.store.save_preferences(self._release_key(release["id"]), merged)
        return copy.deepcopy(merged)

    def album_tag_details(self, release):
        self.check()
        saved = self.store.preferences(self._release_key(release["id"]))
        if (
            self._consistent(release, saved)
            and saved.get("tracks_loaded")
            and saved.get("tag_credits_checked")
            and saved.get("metadata_schema") == 3
            and self._fresh(saved, "tag_checked_at", 1)
        ):
            self.progress(
                f"Using saved release details · {release.get('title') or release['id']}"
            )
            return self._combine(release, saved)
        result = super().album_tag_details(release)
        self.check()
        result.update(tag_checked_at=time.time(), metadata_schema=3)
        self.store.save_preferences(self._release_key(release["id"]), result)
        return copy.deepcopy(result)

    def track_tag_details(self, track):
        self.check()
        key = f"tag-track:{self.market}:{track['id']}"
        saved = self.store.preferences(key)
        if self._fresh(saved, "checked_at", 1) and isinstance(saved.get("track"), dict):
            self.progress(
                f"Using saved track metadata · {track.get('title') or track['id']}"
            )
            return copy.deepcopy(saved["track"])
        result = super().track_tag_details(track)
        self.check()
        self.store.save_preferences(key, dict(checked_at=time.time(), track=result))
        return copy.deepcopy(result)
