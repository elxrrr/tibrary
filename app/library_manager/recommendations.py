"""Conservative, explainable release suggestions; never an authenticity certificate."""
from .core import norm


def text(value):
    if isinstance(value, dict): return value.get('name') or value.get('text') or ''
    return str(value or '')


def recommend(release, local_tracks, artist_names, linked_catalogue=False, artist_ids=(), profile=None):
    from .view_data import normalize_copyright
    profile = profile if profile is not None else reference_profile(local_tracks)
    labels, rights = profile['labels'], profile['rights']
    label = norm(text(release.get('label')))
    copyright = normalize_copyright(text(release.get('copyright')))
    label_match = bool(label and label in labels)
    rights_match = bool(copyright and copyright in rights)
    credits = {norm(text(a)) for a in release.get('album_artists', []) if text(a)}
    credit_ids = set(map(str, release.get('album_artist_ids') or []))
    names = {norm(n) for n in artist_names}
    primary = bool(credits & names or credit_ids & set(map(str, artist_ids)))
    conflict = bool((credits or credit_ids) and not primary)
    local_isrc = profile['isrc']
    overlap = {str(t.get('isrc') or '').replace('-', '').upper() for t in release.get('tracks', [])} & local_isrc
    known_people = profile['people'] - names
    matched_people, matched_tracks = set(), set()
    for track in release.get('tracks', []):
        shared = people(track) & known_people
        if shared:
            matched_people.update(shared)
            # Never count the same recording twice across duplicate editions.
            identity = str(track.get('isrc') or track.get('id') or '')
            if identity: matched_tracks.add(identity.replace('-', '').upper())
    label_releases = profile['label_releases'].get(label, set())
    from .tag_io import _lofty
    score, badge, reasons = _lofty.recommendation_score(
        primary, conflict, linked_catalogue, label_match, label_match and len(label_releases) >= 2,
        rights_match, len(overlap), len(matched_tracks), len(matched_people),
        bool(str(release.get('type') or '').upper() == 'COMPILATION' or release.get('is_compilation')),
        release.get('official') is False, release.get('official') is True)
    if matched_people:
        reasons.append('Shared credits: ' + ', '.join(sorted(matched_people)))
    return dict(score=score, badge=badge, evidence=reasons)


def people(track):
    """Only explicit creative/person credits; never labels or page-owner inference."""
    roles = {'composer', 'lyricist', 'songwriter', 'writer', 'producer', 'remixer', 'performer'}
    result = set()
    for credit in track.get('credits') or []:
        if isinstance(credit, dict) and str(credit.get('role', '')).casefold() in roles:
            result.add(norm(text(credit)))
    for role in roles | {'artists'}:
        values = track.get(role) or track.get('tags', {}).get(role) or []
        if not isinstance(values, list): values = [values]
        result.update(norm(text(v)) for v in values)
    return result - {'', 'unknown', 'various artists'}


def reference_profile(tracks):
    """Build once per linked artist group, rather than once per candidate release."""
    from .view_data import normalize_copyright
    from collections import defaultdict
    labels = defaultdict(set)
    for track in tracks:
        release = str(track.get('tidal_album_id') or track.get('album') or '')
        if release: labels[norm(text(track.get('label')))].add(release)
    return dict(labels={norm(text(t.get('label'))) for t in tracks} - {''},
                rights={normalize_copyright(text(t.get('copyright'))) for t in tracks} - {''},
                isrc={str(t.get('isrc') or '').replace('-', '').upper() for t in tracks} - {''},
                people=set().union(*(people(t) for t in tracks)), label_releases=labels)
