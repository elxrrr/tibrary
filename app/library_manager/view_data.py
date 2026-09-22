"""Build library display data off the GUI thread, using bulk database reads."""
import calendar
import json
import re
from collections import defaultdict
from datetime import datetime, date
from .core import title_key,base_title,coverage,is_compilation_artist


def normalize_copyright(val):
    if isinstance(val, dict):
        val = val.get('text', '')
    if not isinstance(val, str):
        return ''
    cleaned = re.sub(r'©|℗|\([cpCP]\)|\[[cpCP]\]|\{[cpCP]\}|\b[cpCP](?=\s+(?:19|20)\d{2}\b)', ' ', val)
    cleaned = re.sub(r'\b(19\d\d|20\d\d)\b', ' ', cleaned)
    return ' '.join(re.sub(r'[^\w\s]', ' ', cleaned).casefold().split())


def copyright_matches(release_copy, known_copies):
    norm_rel = normalize_copyright(release_copy)
    if not norm_rel:
        return None
    stop_words = {'under', 'exclusive', 'license', 'to', 'records', 'recordings', 'llc', 'ltd', 'inc', 'music', 'group', 'the'}
    rel_words = set(norm_rel.split()) - stop_words
    for known in known_copies:
        norm_k = normalize_copyright(known)
        if not norm_k:
            continue
        if norm_rel in norm_k or norm_k in norm_rel:
            return True
        k_words = set(norm_k.split()) - stop_words
        if rel_words and k_words and (rel_words & k_words):
            return True
    return False


def release_date(value):
    value=str(value or '')[:10]
    try:
        if len(value)==4:value+='-12-31'
        elif len(value)==7:
            year,month=map(int,value.split('-'));value+=f'-{calendar.monthrange(year,month)[1]:02d}'
        return date.fromisoformat(value).isoformat()
    except ValueError:return None


def release_is_out(release):
    date=release_date(release.get('date'))
    return bool(date and date<=datetime.now().date().isoformat() and not release.get('is_prerelease',False))


def build_view(store,market):
    local=store.artists(include_compilations=True)
    # Compare with durable recording links, without rewriting the user's tags.
    # The scan's size/mtime must still agree; a changed file loses stale hints.
    indexed={r['path']:(r['size'],r['mtime']) for r in store.rows('SELECT path,size,mtime FROM local_files WHERE present=1')}
    associations={}
    for record in store.rows('SELECT path,stamp,payload FROM track_links WHERE market=?',(market,)):
        stamp=json.loads(record['stamp']);payload=json.loads(record['payload'])
        if len(stamp)>=4 and indexed.get(record['path'])==(stamp[2],stamp[3]) and payload.get('ids'):
            associations[record['path']]=payload['ids']
    for tracks in local.values():
        for track in tracks:
            ids=associations.get(track['path'])
            if ids:track.update(tidal_album_id=ids['album_id'],tidal_track_id=ids['track_id'])
    artists={name:tracks for name,tracks in local.items() if not is_compilation_artist(name)}
    artist_copyrights = defaultdict(set)
    for name, tracks in artists.items():
        for t in tracks:
            c = t.get('copyright')
            if c: artist_copyrights[name].add(c)
    mappings={r['artist']:r for r in store.rows('SELECT * FROM mappings')}
    reviews={r['artist']:r for r in store.rows('SELECT * FROM match_reviews')}
    links=store.linked_mappings();ids={r['tidal_id'] for r in links}
    cached={r['artist_id']:json.loads(r['payload']) for r in store.rows('SELECT artist_id,payload FROM catalogue WHERE market=?',(market,)) if r['artist_id'] in ids}
    types={};releases={};owned_dates={};indexes={}
    for name,tracks in artists.items():
        index=defaultdict(dict)
        for t in tracks:
            for key in [('title',title_key(t.get('album',''))),('base',base_title(t.get('album',''))),('id',str(t.get('tidal_album_id','')))]:index[key][t['path']]=t
        indexes[name]=index
    for mapping in links:
        name=mapping['artist'];ident=mapping['tidal_id']
        if name not in artists:continue
        for track in artists[name]:
            date=release_date(track.get('date',''))
            if date and date<=datetime.now().date().isoformat():owned_dates.setdefault(ident,set()).add(date)
        artist=cached.get(ident)
        if not artist:continue
        types.setdefault(name,{}).update({title_key(r['title']):r.get('type') or 'Unknown' for r in artist.get('releases',[])})
        for release in artist.get('releases',[]):
            if not release_is_out(release):continue
            entry=releases.setdefault(str(release['id']),dict(release=release,artist_id=artist['id'],local_artists=set(),artist_ids=set()))
            entry['local_artists'].add(name);entry['artist_ids'].add(artist['id'])
            if release.get('tracks_loaded'):entry['release']=release
    compared=[]
    for entry in releases.values():
        release=entry['release'];tracks={}
        for name in entry['local_artists']:
            for key in [('title',title_key(release['title'])),('base',base_title(release['title'])),('id',str(release['id']))]:tracks.update(indexes[name].get(key,{}))
        item=coverage(list(tracks.values()),dict(releases=[release]))[0]
        item.update(artist_id=entry['artist_id'],artist_ids=sorted(entry['artist_ids']),local_artists=sorted(entry['local_artists']))
        rel_copy = release.get('copyright')
        known = set().union(*(artist_copyrights.get(name, set()) for name in entry['local_artists']))
        copy_match = copyright_matches(rel_copy, known) if known and rel_copy else None
        item['copyright_match'] = copy_match
        item['copyright'] = rel_copy.get('text', '') if isinstance(rel_copy, dict) else (rel_copy or '')
        from .recommendations import recommend
        item['recommendation'] = recommend(release, [t for name in entry['local_artists'] for t in artists.get(name, [])], entry['local_artists'], linked_catalogue=True)
        compared.append(item)
        if item['state'] in ('Owned complete','Owned partial','Present locally'):
            if release.get('date'):
                for ident in item['artist_ids']:owned_dates.setdefault(ident,set()).add(release['date'])
            if rel_copy:
                for name in entry['local_artists']:
                    artist_copyrights[name].add(rel_copy.get('text', '') if isinstance(rel_copy, dict) else str(rel_copy))
    from .link_statistics import link_statistics
    stats=link_statistics(store,market)
    library_counts={r['root']:dict(root=r['root'],track_count=0,linked_tracks=0) for r in store.rows('SELECT root FROM roots')}
    for record in store.rows('SELECT path,root FROM local_files WHERE present=1 AND metadata IS NOT NULL'):
        counts=library_counts.setdefault(record['root'],dict(root=record['root'],track_count=0,linked_tracks=0))
        counts['track_count']+=1;counts['linked_tracks']+=int(record['path'] in stats['active'])
    return dict(library_link_counts=list(library_counts.values()),link_statistics=stats,artists=artists,mappings=mappings,reviews=reviews,types=types,compared=compared,owned_dates=owned_dates,
                artist_copyrights=dict(artist_copyrights),
                track_count=sum(map(len,local.values())),release_count=sum(len({title_key(t.get('album','')) for t in tracks}) for tracks in local.values()),
                roots=store.rows('SELECT * FROM roots ORDER BY root'),scans=store.rows('SELECT * FROM scans ORDER BY id DESC LIMIT 6'),
                queue=store.rows('SELECT * FROM queue ORDER BY updated DESC'),caches=store.rows('SELECT fetched FROM catalogue WHERE market=? ORDER BY fetched',(market,)))


def filter_coverage(data,decisions,filters,demo=False):
    from .discovery import working_releases,release_groups
    timeline,query,status,kind = filters[:4]
    copyright_filter = filters[4] if len(filters) > 4 else 'All copyrights'
    recommendation_filter = filters[5] if len(filters) > 5 else 'All recommendations'
    compared=data['compared'];owned_dates=data['owned_dates'];rows=[]
    dates_cache={}
    available_candidates=[]
    for original in compared:
        item=dict(original);release=item['release'];ids=tuple(item['artist_ids'])
        if ids not in dates_cache:dates_cache[ids]=sorted(set().union(*(owned_dates.get(ident,set()) for ident in ids)),reverse=True)
        dates=dates_cache[ids]
        if decisions.get(str(release['id']))=='ignored':item['state']='Ignored'
        elif decisions.get(str(release['id']))=='queued':item['state']='Queued'
        if item['state']=='Missing release' and decisions.get(str(release['id'])) not in ('queued', 'ignored'):
            if timeline=='Newer than newest owned' and (not dates or release.get('date','')<=dates[0]):
                pass
            elif timeline=='Between newest two owned' and (len(dates)<2 or not dates[1]<release.get('date','')<dates[0]):
                pass
            elif timeline=='Incomplete albums' and (release.get('type') not in ('ALBUM','EP')):
                pass
            else:
                available_candidates.append(original)
        if timeline=='All missing releases' and item['state'] not in ('Missing release','Owned partial','Queued'):continue
        if timeline=='Incomplete albums' and (release.get('type') not in ('ALBUM','EP') or item['state'] not in ('Owned partial','Present locally')):continue
        if timeline=='Newer than newest owned' and (not dates or release.get('date','')<=dates[0]):continue
        if timeline=='Between newest two owned' and (len(dates)<2 or not dates[1]<release.get('date','')<dates[0]):continue
        if query.casefold() not in (release['title']+release['artist']).casefold():continue
        if status not in ('All statuses',item['state']):continue
        if kind not in ('All types',release.get('type')):continue
        if copyright_filter == 'Matching local copyrights' and item.get('copyright_match') is not True:
            continue
        if copyright_filter == 'No copyright match' and item.get('copyright_match') is not False:
            continue
        badge=item.get('recommendation',{}).get('badge','Potential')
        if recommendation_filter == 'Recommended' and badge!='Recommended':continue
        if recommendation_filter == 'Potential' and badge!='Potential':continue
        if recommendation_filter == 'Suspect / Low match' and badge!='Suspect':continue
        if recommendation_filter == 'Unmatched' and badge!='Unmatched':continue
        rows.append(item)
    available_grouped=[i for group in release_groups(available_candidates) if not any(decisions.get(str(r['release']['id'])) in ('queued','ignored') for r in group) for i in group]
    available_count=len(working_releases(available_grouped,demo,data.get('link_cache_days',30)))
    # Recommendations are advisory.  Keep every catalogue candidate available
    # for inspection/override; availability and recommendation filters decide
    # what the user sees, rather than deleting low-confidence rows here.
    visible=sorted(rows,key=lambda r:r['release'].get('date',''),reverse=True)
    return rows,visible,available_count
