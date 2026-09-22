"""Recording-verified tag proposals; changing files remains a separate action."""
import copy
import json
import time
from pathlib import Path
from collections import defaultdict
from .core import title_key, norm, is_compilation_artist
from .maintenance import first, normalized_date
from .tidal import CatalogueError


def _equivalent_edition_key(release):
    """Return a conservative identity for duplicate catalogue album IDs.

    online source can expose the same product under more than one album ID. Those IDs
    are interchangeable for linking only when the complete release structure,
    recording sequence agree. A differing product code is allowed only when
    every recording has the same ISRC and duration; product metadata remains
    attached to its own edition. Rolling releases deliberately
    remain distinct because their track sequence or total differs.
    """
    tracks = release.get('tracks') or []
    if not release.get('tracks_loaded') or not tracks:
        return None
    try:
        expected = int(release.get('track_count') or len(tracks))
    except (TypeError, ValueError):
        return None
    if expected != len(tracks):
        return None
    positioned = []
    for track in tracks:
        try:
            disc = int(track.get('disc_number') or 1)
            number = int(track.get('track_number') or 0)
        except (TypeError, ValueError):
            return None
        if not number:
            return None
        isrc = ''.join(ch for ch in str(track.get('isrc') or '').upper() if ch.isalnum())
        duration = track.get('duration')
        try:
            duration = round(float(duration), 1) if duration else 0
        except (TypeError, ValueError):
            duration = 0
        credits=tuple(sorted(norm(a.get('name','') if isinstance(a,dict) else a) for a in track.get('artists',[]) if a))
        positioned.append((disc, number, title_key(track.get('title', '')), isrc, duration, credits, bool(track.get('explicit'))))
    # Unknown duration is not proof of audio-equivalent editions.
    if any(not p[4] for p in positioned):return None
    if len({p[:2] for p in positioned}) != len(positioned):
        return None
    barcode = ''.join(ch for ch in str(release.get('barcode') or release.get('upc') or '') if ch.isalnum())
    # Without a product code, require every recording to carry a stable ISRC.
    if not barcode and not all(p[3] for p in positioned):
        return None
    credits = tuple(norm(a) for a in release.get('album_artists', []) if norm(a))
    return (title_key(release.get('title', '')), normalized_date(release.get('date', '')),
            str(release.get('type') or '').casefold(), bool(release.get('explicit')),
            int(release.get('disc_count') or max(p[0] for p in positioned)), expected,
            '' if all(p[3] and p[4] for p in positioned) else barcode, tuple(sorted(positioned)))


def _collapse_equivalent_editions(options, details, preferred_id=None):
    """Keep one working edition while retaining every exact catalogue alias."""
    groups = {}
    edition_keys={}
    for option in options:
        ident = str(option.get('id', ''))
        key = _equivalent_edition_key(details.get(ident, {}))
        if key is None:continue
        # Group by recording sequence, then enforce duration tolerance against
        # every member: transitive chains must not merge distinct versions.
        identity=key[:7]+(tuple(t[:4]+t[5:] for t in key[7]),)
        edition_keys[ident]=key
        matching=None
        for group_key,identifiers in groups.items():
            if group_key[0]!=identity:continue
            if all(all(abs(a[4]-b[4])<=(3 if a[3] and b[3] else 0) for a,b in zip(key[7],edition_keys[other][7])) for other in identifiers):
                matching=group_key;break
        if matching is None:matching=(identity,len(groups));groups[matching]=[]
        if ident not in groups[matching]:groups[matching].append(ident)
    aliases = {};members = {}
    for identifiers in groups.values():
        if len(identifiers) < 2:
            continue
        if preferred_id is not None and str(preferred_id) in identifiers and (details.get(str(preferred_id),{}).get('available') is True or not any(details.get(i,{}).get('available') is True for i in identifiers)):
            canonical = str(preferred_id)
        else:
            def rank(ident):
                release = details.get(ident, {})
                numeric = int(ident) if ident.isdecimal() else float('inf')
                return (release.get('available') is not True,
                        -(release.get('link_checked_at') or 0), numeric, ident)
            canonical = min(identifiers, key=rank)
        aliases.update({ident: canonical for ident in identifiers if ident != canonical})
        members[canonical]=set(identifiers)
    collapsed=[]
    for option in options:
        ident=str(option.get('id',''))
        if ident in aliases:continue
        equivalent=members.get(ident,{ident})
        placements=[]
        for other in options:
            if str(other.get('id','')) in equivalent:
                placement=dict(album_id=str(other.get('id','')),track_id=str(other.get('track_id','')))
                if placement not in placements:placements.append(placement)
        album_credits={tuple(details.get(a,{}).get('album_artists',[])) for a in equivalent}
        if len(album_credits)>1:
            option=dict(option,canonical_artist_conflict=True,artist_credit_evidence=[dict(album_id=a,album_artists=details.get(a,{}).get('album_artists',[])) for a in sorted(equivalent)],changes={k:v for k,v in option.get('changes',{}).items() if k!='albumartist'})
        if len(placements)>1:
            option=dict(option,equivalent_placements=placements,
                        evidence=option.get('evidence','')+f' · {len(placements)} equivalent recording editions retained')
        collapsed.append(option)
    return collapsed


def check_album_tags(rows, store, market, api, cancelled=lambda: False, progress=lambda s: None, enrich=False, covers=False, correct_metadata=False, dj_lookup=None, on_result=lambda row:None, force=False, context_rows=None, cache_first=False):
    result = copy.deepcopy(rows)
    catalogues = store.rows('SELECT artist_id,payload FROM catalogue WHERE market=?', (market,))
    releases = {}; owners=defaultdict(list)
    for cache in catalogues:
        cache['decoded']=json.loads(cache['payload'])
        for release in cache['decoded'].get('releases', []):
            releases.setdefault(str(release['id']), release)
            owners[str(release['id'])].append(cache)
    by_title=defaultdict(list);by_artist_date=defaultdict(list)
    by_isrc=defaultdict(dict);by_track=defaultdict(dict)
    for release in releases.values():
        by_title[title_key(release.get('title',''))].append(release)
        by_artist_date[(norm(release.get('artist','')),release.get('date'))].append(release)
    def index_tracks(release):
        for track in release.get('tracks',[]):
            isrc=(track.get('isrc') or '').replace('-','').upper()
            if isrc:by_isrc[isrc][str(release['id'])]=release
            by_track[str(track['id'])][str(release['id'])]=release

    for release in releases.values():index_tracks(release)

    from .release_matching import group_key, structure, recording_matches, total, position
    local_groups = defaultdict(list)
    seen_local=set()
    for row in (context_rows if context_rows is not None else result):
        if row['path'] not in seen_local:
            local_groups[group_key(row)].append(row);seen_local.add(row['path'])
    structures = {}

    cache_days=store.preferences('release_links', {'max_age_days':30}).get('max_age_days',30)
    def cache_current(release):
        # Linking reuses structural evidence for the configured discovery window.
        # Tag/artwork enrichment keeps its shorter freshness window.
        seconds=86400 if (enrich or covers or correct_metadata) and not cache_first else float(cache_days)*86400
        return not force and (seconds==0 or time.time()-release.get('tag_checked_at',0)<seconds)

    fetched = {}
    for cached in store.rows('SELECT payload FROM app_preferences WHERE key LIKE ?', (f'tag-review:{market}:%',)):
        release = json.loads(cached['payload'])
        if cache_current(release):
            fetched[str(release['id'])] = release
            ident=str(release['id'])
            for group,key in ((by_title,title_key(release.get('title',''))),(by_artist_date,(norm(release.get('artist','')),release.get('date')))):
                group[key]=[r for r in group[key] if str(r['id'])!=ident]+[release]
            index_tracks(release)
    lookups = {}; track_details = {}; cover_cache = {}
    def discover(isrc,track_id):
        lookup=(isrc,track_id)
        if lookup not in lookups:
            # Persist discovery, including empty results, across linking runs.
            key=f'recording-discovery:{market}:{isrc}:{track_id}'
            saved=store.preferences(key)
            if not force and time.time()-saved.get('checked_at',0)<86400*7 and isinstance(saved.get('releases'),list):
                lookups[lookup]=saved['releases']
            else:
                lookups[lookup]=api.recording_releases(isrc) if isrc else api.recording_releases(track_id=track_id)
                if isinstance(lookups[lookup],list) and not cancelled():store.save_preferences(key,dict(checked_at=time.time(),releases=lookups[lookup]))
        return lookups[lookup]

    sibling_album_ids = {}
    for r in result:
        rtags = r.get('tags', {})
        aid = (r.get('linked_ids', {}).get('album_id') or 
               first(rtags, 'tidal_album_id') or 
               first(rtags, 'tidalalbumid'))
        if aid and str(aid).isdecimal():
            p_path = Path(r['path']).parent
            p = str(p_path)
            alb = title_key(first(rtags, 'album'))
            if alb:
                sibling_album_ids[(p, alb)] = str(aid)
            sibling_album_ids[p] = str(aid)
            if p_path.name.lower().startswith(('disc', 'cd')):
                gp = str(p_path.parent)
                if alb:
                    sibling_album_ids[(gp, alb)] = str(aid)
                sibling_album_ids[gp] = str(aid)

    # Bulk-load all known sibling album IDs from track_links in a single fast query
    try:
        for sr in store.rows("SELECT path, payload FROM track_links WHERE market=?", (market,)):
            sp = json.loads(sr['payload'])
            said = sp.get('ids', {}).get('album_id') or sp.get('catalogue_choice', {}).get('id')
            if said and str(said).isdecimal():
                parent_path = Path(sr['path']).parent
                parent_dir = str(parent_path)
                sibling_album_ids[parent_dir] = str(said)
                if parent_path.name.lower().startswith(('disc', 'cd')):
                    sibling_album_ids[str(parent_path.parent)] = str(said)
    except Exception:
        pass

    # Also load any local_files with saved tidal_album_id in tags
    try:
        for lr in store.rows("SELECT path, metadata FROM local_files WHERE present=1 AND metadata LIKE '%tidal_album_id%'"):
            ltags = json.loads(lr['metadata'])
            laid = first(ltags, 'tidal_album_id') or first(ltags, 'tidalalbumid')
            if laid and str(laid).isdecimal():
                parent_path = Path(lr['path']).parent
                parent_dir = str(parent_path)
                sibling_album_ids[parent_dir] = str(laid)
                if parent_path.name.lower().startswith(('disc', 'cd')):
                    sibling_album_ids[str(parent_path.parent)] = str(laid)
    except Exception:
        pass

    for index, row in enumerate(result):
        if cancelled(): break
        tags = row.get('tags', {})
        title = first(tags, "title") or Path(row['path']).stem
        artist = first(tags, "artist") or first(tags, "albumartist")
        track_desc = f"{artist} — {title}" if artist else title
        progress(f'Searching for: {track_desc}')
        row.pop('catalogue_choice', None)
        row['catalogue_options'] = []
        if row.get('blocked'):
            row['catalogue_note']='Cannot inspect: '+str(row['blocked'])
            on_result(row)
            continue
        isrc = first(tags, 'isrc').replace('-', '').upper()
        linked=row.get('linked_ids',{})
        track_id = linked.get('track_id') or first(tags,'tidal_track_id') or first(tags,'tidaltrackid')
        equivalent_map={str(p.get('album_id','')):str(p.get('track_id','')) for p in row.get('equivalent_ids',[]) if p.get('album_id')}
        evidence = f'Exact recording ISRC {isrc}' if isrc else (f'Saved online source track ID {track_id}' if track_id else 'Album, track title and duration agree · review required')
        errors = []
        metadata_options = []; dj_checks=[]; cover_options = []; existing_cover = None
        if covers:
            row.pop('artwork_change',None);row.pop('artwork_proposal',None)
            from .artwork import prepare_existing_cover
            try:existing_cover=prepare_existing_cover(row,store)
            except (OSError,ValueError) as exc:progress(str(exc))
            if existing_cover:row['artwork_change']=existing_cover
        album, artist = title_key(first(tags, 'album')), norm(first(tags, 'albumartist'))
        date = normalized_date(first(tags, 'date'))
        candidates = [r for r in by_title.get(album,[]) if r.get('available') is not False and (not date or not r.get('date') or r['date']==date)]
        if artist and date:candidates += [r for r in by_artist_date.get((artist,date),[]) if r.get('available') is not False]
        cached_recordings=list(by_isrc.get(isrc,{}).values())+list(by_track.get(track_id,{}).values())
        candidates = list({str(r['id']):r for r in candidates + cached_recordings}.values())
        album_id=linked.get('album_id') or first(tags,'tidal_album_id') or first(tags,'tidalalbumid')
        cached_candidates = list(candidates)
        sibling_id = None
        if album_id and str(album_id).isdecimal():
            saved=fetched.get(str(album_id))
            saved=(saved if isinstance(saved,dict) else None) or releases.get(str(album_id)) or dict(id=album_id,title=first(tags,'album'),artist=first(tags,'albumartist'),available=None)
            # A saved placement is useful evidence, but it must not hide a
            # structurally exact edition discovered after that link was made.
            candidates=[saved]+[candidate for candidate in candidates if str(candidate['id'])!=str(album_id)]
            for equivalent_album in equivalent_map:
                if equivalent_album==str(album_id) or not equivalent_album.isdecimal():continue
                saved_equivalent=fetched.get(equivalent_album) or releases.get(equivalent_album)
                if saved_equivalent and equivalent_album not in {str(candidate['id']) for candidate in candidates}:
                    candidates.append(saved_equivalent)
        else:
            parent_path = Path(row['path']).parent
            parent_dir = str(parent_path)
            grandparent_dir = str(parent_path.parent) if parent_path.name.lower().startswith(('disc', 'cd')) else None
            sibling_id = (sibling_album_ids.get((parent_dir, album)) or 
                          sibling_album_ids.get(parent_dir) or 
                          (sibling_album_ids.get((grandparent_dir, album)) if grandparent_dir else None) or
                          (sibling_album_ids.get(grandparent_dir) if grandparent_dir else None))
            if sibling_id:
                saved = fetched.get(sibling_id)
                rel_obj = (saved if isinstance(saved, dict) else None) or releases.get(sibling_id) or dict(id=sibling_id, title=first(tags, 'album'), artist=first(tags, 'albumartist'), available=None)
                if str(rel_obj['id']) not in {str(c['id']) for c in candidates}:
                    candidates.insert(0, rel_obj)
        discovered=False
        if not candidates and (isrc or track_id):
            try:
                candidates=discover(isrc,track_id) or [];discovered=True
            except CatalogueError as exc:
                if exc.batch_fatal or exc.status in (401,403,429): raise
                errors.append(str(exc))
                candidates=[]
        # Evaluate every discovered edition. Details are cached per release and
        # cancellation/rate limits remain enforced by the request layer. An
        # arbitrary candidate cap must not strand otherwise exact releases.
        candidates.sort(key=lambda r:title_key(r.get('title',''))!=album)
        for release in candidates:
            if cancelled(): break
            # Once this release is verified, unrelated compilations cannot improve its placement.
            if title_key(release.get('title',''))!=album and any(o.get('structure',{}).get('compatible') and title_key(o.get('album',''))==album for o in row['catalogue_options']):continue
            ident = str(release['id'])
            if release.get('tag_credits_checked') and release.get('tracks_loaded') and cache_current(release):
                fetched[ident] = release
            if (enrich or correct_metadata or covers) and isinstance(fetched.get(ident),dict) and fetched[ident].get('metadata_schema')!=3:fetched.pop(ident)
            if ident not in fetched:
                try:
                    detailed = api.album_tag_details(release)
                    detailed['tag_checked_at'] = time.time()
                    detailed['metadata_schema'] = 3
                    fetched[ident] = detailed
                    index_tracks(detailed)
                    with store.connect() as db:
                        db.execute('INSERT OR REPLACE INTO app_preferences VALUES(?,?)', (f'tag-review:{market}:{ident}', json.dumps(detailed)))
                    # Persist details without replacing each catalogue's artist context.
                    for cache in owners.get(ident,[]):
                        payload = cache['decoded']
                        changed = False
                        for item in payload.get('releases', []):
                            if str(item['id']) == ident:
                                context_artist = item.get('artist')
                                item.update(detailed, artist=context_artist); changed = True
                        if changed:
                            cache['payload'] = json.dumps(payload)
                            with store.connect() as db:
                                db.execute('UPDATE catalogue SET payload=? WHERE artist_id=? AND market=?',
                                           (cache['payload'], cache['artist_id'], market))
                except CatalogueError as exc:
                    if exc.batch_fatal or exc.status in (401,403,429): raise
                    fetched[ident] = str(exc)
            detailed = fetched[ident]
            if isinstance(detailed, str):
                errors.append(detailed); continue
            candidate_track_id=equivalent_map.get(ident,track_id)
            matches = [t for t in detailed.get('tracks', []) if ((t.get('isrc') or '').replace('-', '').upper()==isrc
                       if isrc and t.get('isrc') else candidate_track_id and str(t['id'])==candidate_track_id)]
            is_sibling_album = (sibling_id and str(detailed.get('id')) == str(sibling_id))
            if not isrc and not track_id and title_key(detailed.get('title',''))==album and (is_sibling_album or any(norm(a)==artist for a in detailed.get('album_artists',[]))):
                matches = [t for t in detailed.get('tracks',[]) if title_key(t.get('title',''))==title_key(first(tags,'title')) and row.get('duration',0)>0 and t.get('duration',0)>0 and abs(row['duration']-t['duration'])<=3]
            credits=detailed.get('album_artists',[])
            # A saved placement must agree with the local release title. Track
            # totals are evaluated by the whole-release structure check below,
            # which supports both release-total and per-disc tag conventions.
            saved_conflict=ident==album_id and title_key(detailed.get('title',''))!=album
            candidate_structure = structure(local_groups[group_key(row)], detailed)
            cached_fit = False
            if not candidate_structure['compatible'] or saved_conflict:
                known={str(r['id']) for r in candidates}
                alternatives=[r for r in cached_candidates if str(r['id']) not in known]
                candidates.extend(alternatives)
                cached_fit=any(r.get('tracks') and structure(local_groups[group_key(row)],r)['compatible'] for r in candidates if str(r['id'])!=ident)
            if (len(matches)!=1 or saved_conflict or not candidate_structure['compatible']) and not discovered and not cached_fit and (isrc or track_id):
                discovered=True
                try:
                    more=discover(isrc,track_id)
                    if isinstance(more, (list, tuple)):
                        known={str(r['id']) for r in candidates}
                        additions=[r for r in more if str(r['id']) not in known]
                        # Prefer the album named in the current file, without using its folder.
                        fitting=[r for r in additions if title_key(r.get('title',''))==album]
                        candidates.extend(fitting or additions)
                        if saved_conflict and fitting:continue
                except CatalogueError as exc:
                    if exc.batch_fatal or exc.status in (401,403,429):raise
                    errors.append(str(exc))
            if len(matches) != 1:continue
            track = matches[0]
            if not recording_matches(row, track) and candidate_structure.get('omitted_version_labels',{}).get(row.get('path',''))!=str(track.get('id','')): continue
            structure_key = (group_key(row), ident)
            if structure_key not in structures:
                structures[structure_key] = structure(local_groups[group_key(row)], detailed)
            release_structure = structures[structure_key]
            correction={};option_metadata={}
            if correct_metadata or (enrich and release_structure['compatible']):
                from .enrichment import missing_tags,corrected_tags,needs_extended_metadata
                key=str(track['id'])
                if key not in track_details:
                    saved=store.preferences(f'tag-track:{market}:{key}')
                    if not force and time.time()-saved.get('checked_at',0)<86400:track_details[key]=saved['track']
                    else:
                        try:
                            track_details[key]=api.track_tag_details(track)
                            store.save_preferences(f'tag-track:{market}:{key}',dict(checked_at=time.time(),track=track_details[key]))
                        except CatalogueError as exc:
                            if exc.batch_fatal or exc.status in (401,403,429):raise
                            errors.append('Additional track tags could not be read: '+str(exc))
                            track_details[key]=track
                extra=dict(track_details[key],track_number=track.get('track_number'),disc_number=track.get('disc_number'))
                enriched_release=dict(detailed)
                fallback=None
                if enrich and dj_lookup and needs_extended_metadata({},detailed,extra):
                    fallback=dj_lookup(extra,album_id=ident)
                    # Official values and album-specific track positions take precedence.
                    supplied=missing_tags({},detailed,extra)
                    if not supplied.get('bpm'):extra['bpm']=fallback.get('bpm')
                    if not supplied.get('initialkey'):
                        extra.update({field:fallback.get(field) for field in ('key','key_scale','initialkey','tkey','musical_key')})
                    for field in ('isrc','title','artists','copyright'):
                        if not extra.get(field) and fallback.get(field):extra[field]=fallback[field]
                    for field,value in fallback.get('release',{}).items():
                        if field!='id' and not enriched_release.get(field) and value:enriched_release[field]=value
                if enrich:
                    option_metadata=missing_tags(tags,enriched_release,extra)
                    metadata_options.append((ident,str(track['id']),option_metadata))
                    supplied=missing_tags({},enriched_release,extra)
                    complete=bool(supplied.get('bpm') and supplied.get('initialkey'))
                    dj_checks.append(dict(track_id=key,album_id=ident,bpm=supplied.get('bpm',[]),key=supplied.get('initialkey',[]),
                                          status='checked' if complete or fallback else 'incomplete',
                                          note='API metadata checked (cached responses reused)' if complete or fallback else 'BPM/key check incomplete · connect the subscriber metadata account or retry'))
                if correct_metadata:correction=corrected_tags(tags,detailed,extra)
            if covers and release_structure['compatible'] and not existing_cover and row.get('cover_size')!=(1280,1280):
                from .artwork import prepare_cover
                if ident not in cover_cache:
                    try:cover_cache[ident]=prepare_cover(detailed,store,cancelled)
                    except CatalogueError as exc:
                        progress(str(exc));cover_cache[ident]=None
                cover_options.append(cover_cache[ident])
            credits = detailed.get('album_artists', [])
            if detailed.get('credits_complete') is False:
                errors.append('Some album artist credits are unavailable; no album-artist correction proposed.')
                continue
            if not credits: continue
            # Preserve explicit multi-artist album credits instead of choosing a featured artist.
            credit = ', '.join(credits)
            proposed = {'albumartist': [credit], 'album': [detailed['title']]}
            if correct_metadata:proposed.update(correction,tidal_track_id=[str(track['id'])],tidal_album_id=[ident])
            aligned=release_structure.get('alignments',{}).get(row.get('path',''),{})
            position_changed=bool(aligned and (int(aligned.get('track_number') or 0)!=position(row)[1]
                                               or int(aligned.get('disc_number') or 1)!=position(row)[0]))
            totals_need_repair=row.get('path','') in release_structure.get('total_repairs',[])
            if title_key(detailed['title']) != album or (correct_metadata and (position_changed or totals_need_repair)):
                if not track.get('track_number') or not track.get('disc_number'):
                    errors.append('Matching recording found, but album track/disc numbering is unavailable; no album retag proposed.')
                    continue
                matched_track=track
                if aligned.get('track_id'):
                    matched_track=next((candidate for candidate in detailed.get('tracks',[])
                                        if str(candidate.get('id',''))==str(aligned['track_id'])),track)
                proposed.update(tracknumber=[f"{int(matched_track['track_number']):02d}"], discnumber=[f"{int(matched_track['disc_number']):02d}"])
                if detailed.get('disc_count'): proposed['disctotal'] = [f"{int(detailed['disc_count']):02d}"]
                disc_tracks = [t for t in detailed.get('tracks', []) if t.get('disc_number') == matched_track['disc_number']]
                if disc_tracks: proposed['tracktotal'] = [f'{len(disc_tracks):02d}']
                if normalized_date(detailed.get('date', '')): proposed['date'] = [detailed['date']]
                if 'totaldiscs' in tags and 'disctotal' in proposed: proposed['totaldiscs'] = proposed['disctotal']
                if 'totaltracks' in tags and 'tracktotal' in proposed: proposed['totaltracks'] = proposed['tracktotal']
            alternatives = [credit] + (credits if len(credits) > 1 else [])
            for album_artist in alternatives:
                alternative = dict(proposed, **option_metadata)
                alternative['albumartist']=[album_artist]
                edits = {k:v for k,v in alternative.items() if tags.get(k) != v}
                row['catalogue_options'].append(dict(id=ident, track_id=str(track['id']), recording_verified=bool(isrc or track_id), structure=release_structure, release_counts=dict(tracktotal=sum(int(t.get('disc_number') or 1)==int(track.get('disc_number') or 1) for t in detailed.get('tracks',[])), disctotal=detailed.get('disc_count') or max((int(t.get('disc_number') or 1) for t in detailed.get('tracks',[])),default=1)), position_label=f"Disc {int(track.get('disc_number') or 1):02d}/{detailed.get('disc_count') or '?'} · Track {int(track.get('track_number') or 0):02d}/{len(detailed.get('tracks',[]))} (release total)", artist=album_artist, album=detailed['title'], changes=edits,
                                                     whole_release_verified=bool(release_structure.get('compatible') and release_structure.get('matched')==len(local_groups[group_key(row)])),
                                                     credit_label='Full online source album credit' if album_artist==credit else 'Individual grouping · your choice',
                                                     evidence=f'{evidence} · online source album {ident}'))
        edition_preference=album_id or sibling_id
        local_product=''.join(ch for ch in (first(tags,'upc') or first(tags,'barcode')) if ch.isalnum())
        if not edition_preference and local_product:
            product_ids={str(o['id']) for o in row['catalogue_options']
                         if ''.join(ch for ch in str(fetched.get(str(o['id']),{}).get('barcode') or fetched.get(str(o['id']),{}).get('upc') or '') if ch.isalnum())==local_product}
            if len(product_ids)==1:edition_preference=next(iter(product_ids))
        row['catalogue_options'] = _collapse_equivalent_editions(
            row['catalogue_options'], fetched, preferred_id=edition_preference)
        options = [o for o in row['catalogue_options'] if o.get('structure', {}).get('compatible', True)]
        identity_options = []
        seen_identities = set()
        for option in options:
            identity = (str(option.get('id', '')), str(option.get('track_id', '')))
            if identity not in seen_identities:
                seen_identities.add(identity)
                identity_options.append(option)
        # Identical reissues can share one proposal; competing placements require a choice.
        choices = {json.dumps(o['changes'], sort_keys=True) for o in options}
        sibling_match = next((o for o in options if (album_id or sibling_id) and str(o['id']) == str(album_id or sibling_id)), None)
        if sibling_match and title_key(sibling_match.get('album', '')) == album and not errors and not cancelled() and (sibling_match.get('recording_verified') or sibling_match.get('whole_release_verified')):
            row['catalogue_choice'] = sibling_match
            row['catalogue_note'] = sibling_match['evidence'] + ' · release linked automatically' + (' · matched sibling tracks in folder' if sibling_match['changes'] else ' · tags agree with album folder')
        elif len(options) == 1 and not errors and not cancelled() and (options[0].get('recording_verified') or options[0].get('whole_release_verified')):
            row['catalogue_choice'] = options[0]
            row['catalogue_note'] = options[0]['evidence'] + (' · repair proposed' if options[0]['changes'] else ' · tags agree')
        elif not errors and not cancelled() and len(identity_options) == 1 and (identity_options[0].get('recording_verified') or identity_options[0].get('whole_release_verified')):
            # Album-artist grouping variants are tag-review choices, not
            # competing recording placements. The central link is unambiguous.
            row['catalogue_note'] = identity_options[0]['evidence'] + ' · release linked automatically'
            if len(options) > 1:
                row['catalogue_note'] += f' · {len(options)} album-artist tag choices remain'
        elif not errors and not cancelled() and len(identity_options) > 1 and any(o.get('recording_verified') or o.get('whole_release_verified') for o in identity_options):
            local_alb = first(tags, 'album')
            local_yr = normalized_date(first(tags, 'date'))[:4] if normalized_date(first(tags, 'date')) else ''
            local_tot = first(tags, 'tracktotal') or first(tags, 'totaltracks')
            local_discs = first(tags, 'disctotal') or first(tags, 'totaldiscs')
            scored = []
            for o in identity_options:
                score = 0
                rel_detail = fetched.get(str(o['id']), {})
                o_title = o.get('album', '')
                if local_alb and o_title.casefold() == local_alb.casefold():
                    score += 40
                elif local_alb and title_key(o_title) == title_key(local_alb):
                    score += 25
                o_yr = normalized_date(rel_detail.get('date', ''))[:4] if normalized_date(rel_detail.get('date', '')) else ''
                if local_yr and o_yr == local_yr:
                    score += 20
                if local_tot and local_tot.isdigit():
                    t_count = len(rel_detail.get('tracks', [])) or rel_detail.get('track_count')
                    if t_count == int(local_tot):
                        score += 25
                if local_discs and local_discs.isdigit():
                    d_count = rel_detail.get('disc_count')
                    if d_count == int(local_discs):
                        score += 15
                if o.get('recording_verified'):
                    score += 20
                scored.append((score, o))
            scored.sort(key=lambda x: x[0], reverse=True)
            if scored and scored[0][0] >= 60 and (len(scored) == 1 or scored[0][0] - scored[1][0] >= 15):
                best = scored[0][1]
                row['catalogue_choice'] = best
                row['catalogue_note'] = best['evidence'] + (' · auto-matched best release edition' if best['changes'] else ' · tags agree with best release match')
            else:
                row['catalogue_note'] = f'{len(identity_options)} verified release placements · choose one before proposing changes'
        elif options:
            row['catalogue_note'] = f'{len(options)} verified album/artist choices · choose a placement before proposing changes'
        else:
            row['catalogue_note'] = ('No recording-verified release found. Check the ISRC or saved online source track/album IDs in Inspect all tags, then Verify tags with online source.' if isrc or track_id else 'No recording identifiers found. Match the Album Artist, then Verify tags with online source to review album/title/duration candidates.')+' Existing tags retained.'
        if not options and row['catalogue_options']:
            row['catalogue_note'] = 'Recording found, but release totals or track positions differ · choose an edition manually'
        if row.get('catalogue_choice'):
            info = row['catalogue_choice'].get('structure', {})
            if info.get('incomplete'):
                row['catalogue_note'] += f" · Incomplete Local Album · {info['matched']}/{info['total']} tracks"
            matched_aid = str(row['catalogue_choice']['id'])
            p_path = Path(row['path']).parent
            p_dir = str(p_path)
            if album:
                sibling_album_ids[(p_dir, album)] = matched_aid
            sibling_album_ids[p_dir] = matched_aid
            gp_dir = str(p_path.parent) if p_path.name.lower().startswith(('disc', 'cd')) else None
            if gp_dir:
                if album:
                    sibling_album_ids[(gp_dir, album)] = matched_aid
                sibling_album_ids[gp_dir] = matched_aid
        if enrich:
            row['dj_checks']=dj_checks
            dictionaries=[item[2] for item in metadata_options]
            conflicts=[]
            choice=row.get('catalogue_choice') or (identity_options[0] if len(identity_options)==1 else None)
            if choice:
                allowed={(str(choice.get('id','')),str(choice.get('track_id','')))}
                allowed.update((str(p.get('album_id','')),str(p.get('track_id','')))
                               for p in choice.get('equivalent_placements',[]))
                dictionaries=[values for aid,tid,values in metadata_options if (aid,tid) in allowed]
                common={}
                for key in {key for values in dictionaries for key in values}:
                    supplied={json.dumps(values[key],sort_keys=True) for values in dictionaries if values.get(key)}
                    if len(supplied)==1:common[key]=json.loads(next(iter(supplied)))
                    elif len(supplied)>1:conflicts.append(key)
            else:
                common=dict(dictionaries[0]) if dictionaries else {}
                common={k:v for k,v in common.items() if all(option.get(k)==v for option in dictionaries)}
            # Identification fields from alternate albums must be explicitly chosen.
            if not (isrc or track_id):common={}
            # Album/performer credits never cross-fill from equivalent IDs.
            common.pop('albumartist',None);common.pop('artist',None)
            row['metadata_changes'] = common if not errors and not cancelled() else {}
            row['enrich_enabled'] = True
            row['catalogue_note'] += f' · {len(row["metadata_changes"])} missing tags supplied'
            if conflicts:row['catalogue_note'] += ' · Conflicting equivalent-edition metadata retained for review: '+', '.join(sorted(conflicts))
            unavailable=[k for k in ('genre','label') if not first(tags,k) and k not in row['metadata_changes']]
            if not (first(tags,'bpm') or first(tags,'tempo') or row['metadata_changes'].get('bpm')):unavailable.insert(0,'BPM')
            if not (first(tags,'key') or first(tags,'initialkey') or first(tags,'tkey') or row['metadata_changes'].get('initialkey')):unavailable.insert(0,'musical key')
            if unavailable:row['catalogue_note'] += ' · Not supplied or ambiguous: '+', '.join(unavailable)
        if covers:
            row.pop('artwork_change',None)
            if existing_cover:
                row['artwork_change']=existing_cover
                row['catalogue_note'] += ' · Existing front cover → 1280 × 1280'
            elif cover_options and all(p and p['sha256']==cover_options[0]['sha256'] for p in cover_options) and not errors and not cancelled():
                row['artwork_change']=cover_options[0]
                row['catalogue_note'] += ' · Front cover → 1280 × 1280'
        if errors: row['catalogue_note'] += ' · ' + '; '.join(dict.fromkeys(errors))
        if cancelled(): row['catalogue_note'] += ' · Check cancelled; results may be incomplete'
        if not cancelled() or row.get('catalogue_choice') or row.get('catalogue_options') or row.get('linked_ids'):
            on_result(row)
    return result
