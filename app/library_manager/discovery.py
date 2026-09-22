"""Conservative release grouping and cached, market-specific link checks."""
import time
from .core import title_key


DEFAULT_LINK_CACHE_DAYS = 30


def checked_recently(release, max_age_days=DEFAULT_LINK_CACHE_DAYS):
    checked = release.get('link_checked_at') or 0
    age = time.time() - checked
    return (checked > 0 and age >= 0 and isinstance(release.get('available'), bool)
            and (max_age_days == 0 or age < max_age_days * 86400))


def release_groups(items):
    groups = {}
    for item in items:
        r = item['release']
        # Keep differing editions, dates, track counts and explicit versions apart.
        key = (tuple(item['local_artists']), title_key(r['title']), r.get('date'),
               r.get('type'), r.get('track_count'), r.get('explicit'))
        if r.get('track_count') is None or r.get('explicit') is None:
            key += (r['id'],)
        groups.setdefault(key, []).append(item)
    return list(groups.values())


def working_releases(items, demo=False, max_age_days=DEFAULT_LINK_CACHE_DAYS):
    result = []
    for group in release_groups(items):
        working = [i for i in group if i['release'].get('available') is True
                   and (demo or checked_recently(i['release'], max_age_days))]
        if working:
            result.append(max(working, key=lambda i: (bool(i['release'].get('tracks_loaded')), i['release'].get('link_checked_at', 0))))
    return result


def pending_link_groups(items, max_age_days=DEFAULT_LINK_CACHE_DAYS):
    groups=[group for group in release_groups(items) if not working_releases(group, max_age_days=max_age_days)
            and any(not checked_recently(i['release'], max_age_days) for i in group)]
    return sorted(groups,key=lambda group:group[0]['release'].get('date',''),reverse=True)


def verify_releases(items, api, save, cancel, progress, max_requests=None, max_age_days=DEFAULT_LINK_CACHE_DAYS):
    verified = 0
    requested=0
    groups = pending_link_groups(items, max_age_days)
    if max_requests is not None:groups=groups[:max_requests]
    for number, group in enumerate(groups, 1):
        if cancel(): break
        if working_releases(group, max_age_days=max_age_days): continue
        progress(f'Checking release links · {number}/{len(groups)} · {group[0]["release"]["title"]}')
        for item in sorted(group, key=lambda i: i['release'].get('available') is not True):
            if cancel(): return f'Link checks cancelled · {verified} working releases found'
            if checked_recently(item['release'], max_age_days): continue
            if max_requests is not None and requested>=max_requests:
                return f'Batch complete · {requested} requests · {verified} working releases found. Saved results will be reused for the next batch.'
            # Only definitive unavailable responses are cached; access failures,
            # timeouts and rate limits propagate without declaring a link dead.
            release = api.release_availability(item['release'], force=True)
            requested+=1
            save(item, release)
            item['release']=release
            if release.get('available') is True:
                verified += 1
                break
    return f'Link checks {"cancelled" if cancel() else "finished"} · {requested} requests · {verified} working releases found'


def needs_verification(items, max_age_days=DEFAULT_LINK_CACHE_DAYS):
    return any(not working_releases(group, max_age_days=max_age_days) and any(not checked_recently(i['release'], max_age_days) for i in group)
               for group in release_groups(items))
