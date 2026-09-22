"""Sequential bulk matching with durable per-artist results and bounded work."""
from .core import norm, resolve, title_key
from difflib import SequenceMatcher
import json
from .tidal import CatalogueError
from .credentials import CredentialError


def name_key(value):
    return ''.join(c for c in norm(value) if c.isalnum())


def score_candidates(name, tracks, candidates):
    """Rank identities using independent local releases, not duplicated track counts."""
    scored = resolve(tracks, candidates)[0]
    generic = {'', 'unknown', 'unknown album', 'single', 'singles', 'album',
               'greatest hits', 'best of', 'the best of', 'untitled'}
    local_titles = {title_key(t.get('album', '')) for t in tracks} - generic
    for result in scored:
        artist = result['artist']
        matched = local_titles & {title_key(r['title']) for r in artist['releases']}
        count = len(matched)
        fraction = count / len(local_titles) if local_titles else 0
        exact_name = bool(name_key(name)) and name_key(name) == name_key(artist['name'])
        # One complete release: 75; two: 85; three: 89. More independent
        # releases strengthen the score without multiplying it by track count.
        support = 10 if count == 1 else min(33, 20 + 4 * (count - 2))
        score = min(98, round((40 if exact_name else 15) + 25 * fraction + support)) if count else 0
        result.update(score=score, matched_releases=count, local_releases=len(local_titles),
                      exact_name=exact_name, coverage=fraction, matched_titles=sorted(matched), policy_version=2)
        result['evidence'] = (f'{count}/{len(local_titles)} local release titles match · '
                              f'{"artist name matches" if exact_name else "artist name differs"} · '
                              'track details not checked')
        if artist.get('unverified_summary'):
            result['evidence'] = 'Name candidate only · release summaries not checked'
    scored.sort(key=lambda result: result['score'], reverse=True)
    return scored


def accepted_candidates(scored, preferences):
    if not preferences['enabled']:
        return []
    # Keep every ID with positive local release evidence, including split
    # catalogues. Scores remain explanatory rather than an acceptance gate.
    return [s for s in scored if s.get('matched_releases', 0) > 0
            and not s['artist'].get('unverified_summary')]


def accepts(scored, preferences):
    return bool(accepted_candidates(scored, preferences))


class BatchMatcher:
    def __init__(self, store, api, market, favourites_loader, preferences=None, redact=str):
        self.store, self.api, self.market = store, api, market
        self.favourites_loader = favourites_loader
        self.preferences = dict(preferences or store.match_preferences())
        self.redact = redact
        self.favourites = None
        self.pause_after_review = False

    def candidates(self, name, tracks, cancel, progress):
        self.pause_after_review = False
        if self.favourites is None:
            self.favourites = self.favourites_loader(cancel, progress)
        preferred = [a for a in self.favourites if norm(a['name']) == norm(name)]
        loaded = {}
        deferred = {}
        def fetch(found):
            ranked = sorted(found, key=lambda a: SequenceMatcher(None, name_key(name), name_key(a['name'])).ratio(), reverse=True)
            exact = [a for a in ranked if name_key(a['name']) == name_key(name)]
            shortlist = ranked[:6]
            checked_ids = {a['id'] for a in shortlist}
            for candidate in ranked:
                if candidate['id'] not in checked_ids:
                    deferred[candidate['id']] = dict(candidate, releases=[], unverified_summary=True)
            if len(ranked) > len(shortlist):
                progress(f'Narrowed {len(ranked)} names to {len(shortlist)} closest candidates · remaining names saved for review')
            result=[]
            for candidate in shortlist:
                if cancel(): raise CatalogueError('Matching cancelled.')
                artist = loaded.get(candidate['id']) or self.store.cache(candidate['id'], self.market)
                if artist is None:
                    progress(f'Loading release summaries · {candidate["name"]} (ID {candidate["id"]})')
                    artist = self.api.artist(candidate['id'], progress)
                    self.store.save_catalogue(candidate['id'], self.market, artist)
                else:
                    progress(f'Using cached release summaries · {candidate["name"]}')
                loaded[candidate['id']] = artist
                result.append(artist)
            return result
        fallback_error = ''
        if preferred:
            checked = fetch(preferred)
            scored = score_candidates(name, tracks, checked + list(deferred.values()))
            if set().union(*(set(s['matched_titles']) for s in scored)) >= {title_key(t.get('album', '')) for t in tracks}:
                progress('Favourites account for all local releases · no global search needed')
                return scored, ''
        try:
            found = self.api.search(name)
            combined = {a['id']: a for a in preferred + found}
            checked = fetch(list(combined.values()))
            scored = score_candidates(name, tracks, list(loaded.values()) + [a for ident,a in deferred.items() if ident not in loaded])
            uncovered = {title_key(t.get('album', '')) for t in tracks} - set().union(*(set(s['matched_titles']) for s in scored))
            remaining = [a for ident,a in deferred.items() if ident not in loaded and name_key(a['name']) == name_key(name)]
            while uncovered and remaining:
                progress(f'Checking another artist ID for {len(uncovered)} unmatched local releases')
                fetch(remaining[:6]); remaining = remaining[6:]
                scored = score_candidates(name, tracks, list(loaded.values()) + [a for ident,a in deferred.items() if ident not in loaded])
                uncovered -= set().union(*(set(s['matched_titles']) for s in scored))
            return scored, ''
        except CatalogueError as exc:
            if loaded and not cancel():
                self.pause_after_review = exc.batch_fatal or exc.status in (401,403,429,500,502,503,504)
                fallback_error = self.redact(str(exc))
                progress('Global search failed · keeping favourite candidates for manual review')
                return score_candidates(name, tracks, list(loaded.values())), fallback_error
            raise

    def run(self, artists, cancel=lambda:False, progress=lambda s:None, resume=False, updated=lambda name:None):
        counts = dict(auto=0, review=0, unmatched=0, errors=0, skipped=0)
        completed = 0
        stop = ''
        for index, (name, tracks) in enumerate(artists, 1):
            if cancel(): stop='Cancelled'; break
            progress(f'Artist {index}/{len(artists)} · {name}')
            mapping = self.store.rows('SELECT * FROM mappings WHERE artist=?', (name,))
            previous = self.store.rows('SELECT payload FROM match_reviews WHERE artist=?', (name,))
            saved = json.loads(previous[0]['payload'])['candidates'] if previous else []
            fully_matched = bool(saved and all(s.get('policy_version') == 2 for s in saved) and
                                 {title_key(t.get('album','')) for t in tracks} <= set().union(*(set(s.get('matched_titles',[])) for s in saved)))
            manual_unlinked=bool(mapping and mapping[0]['manual'] and mapping[0]['status']=='unlinked')
            if (mapping and mapping[0]['manual']) or (resume and mapping and mapping[0]['status'] == 'auto' and fully_matched):
                counts['skipped'] += 1
                completed += 1
                progress('Keeping saved decision · select this artist and Match selected to reassess an automatic result')
                continue
            try:
                scored, warning = self.candidates(name, tracks, cancel, progress)
                if cancel(): stop='Cancelled'; break
                accepted = accepted_candidates(scored, self.preferences) if not warning and not manual_unlinked else []
                status = 'auto' if accepted else ('review' if scored else 'unmatched')
                self.store.save_match_review(name, status, scored, self.market, warning)
                best = scored[0] if scored else None
                evidence = best['evidence'] if best else 'No candidate artists found'
                if accepted:
                    evidence += f' · {len(accepted)} TIDAL identities linked'
                self.store.mapping(name, accepted[0]['artist']['id'] if accepted else None, status, evidence,
                                   extra_ids=[s['artist']['id'] for s in accepted[1:]])
                counts[status] += 1
                if self.pause_after_review:
                    stop='Paused after a connection or access failure; favourite candidates saved for review'
                progress(f'{name} · {"Automatically matched" if status == "auto" else "Needs review" if status == "review" else "Unmatched"} · {evidence}')
            except (CatalogueError, CredentialError) as exc:
                if cancel(): stop='Cancelled'; break
                message = self.redact(str(exc))
                self.store.save_match_review(name, 'error', [], self.market, message)
                counts['errors'] += 1
                progress(f'{name} · saved error for review · {message}')
                # Avoid repeatedly submitting the same denied or rate-limited request.
                if isinstance(exc, CredentialError) or getattr(exc, 'batch_fatal', False) or getattr(exc, 'status', None) in (401,403,429,500,502,503,504) or self.favourites is None:
                    stop='Paused after a connection or access failure'
            completed += 1
            updated(name)
            if stop: break
        remaining = len(artists) - completed
        return (f'{stop or "Batch finished"} · {completed}/{len(artists)} processed · '
                f'{counts["auto"]} auto-matched · {counts["review"]} need review · {counts["unmatched"]} unmatched · '
                f'{counts["errors"]} errors · {counts["skipped"]} saved decisions kept · {remaining} remaining. '
                'Match all retries unresolved artists; Match selected reassesses selected automatic results.')
