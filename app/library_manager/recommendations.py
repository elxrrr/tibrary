"""Explainable catalogue recommendations, not authenticity certification."""
from .core import norm


def text(v):
    if isinstance(v, dict): return v.get('name') or v.get('text') or ''
    return str(v or '')


def recommend(release, local_tracks, artist_names, linked_catalogue=False):
    from .view_data import normalize_copyright
    score, reasons = 55, []
    label = norm(text(release.get('label')))
    labels = {norm(text(t.get('label'))) for t in local_tracks if t.get('label')}
    copyright = normalize_copyright(text(release.get('copyright')))
    copyrights = {normalize_copyright(text(t.get('copyright'))) for t in local_tracks if t.get('copyright')}
    if label and label in labels:
        score += 20; reasons.append('Label matches local releases')
    elif label and labels:
        score -= 10; reasons.append('Different label; artists can change labels')
    if copyright and copyright in copyrights:
        score += 15; reasons.append('Copyright holder matches local releases')
    credits = {norm(text(a)) for a in release.get('album_artists', [])}
    primary = norm(text(release.get('artist')))
    if primary: credits.add(primary)
    names = {norm(n) for n in artist_names}
    if credits & names:
        score += 30; reasons.append('Primary album artist credit matches')
    elif credits:
        score -= 30; reasons.append('Artist credit does not match the linked local artist')
    if linked_catalogue:
        score += 10 if credits & names else 5
        reasons.append('Appears in a confirmed artist catalogue')
    if str(release.get('type') or '').upper() == 'COMPILATION' or release.get('is_compilation'):
        score -= 20; reasons.append('Compilation release')
    formats={str(t.get('releasetype') or '').upper() for t in local_tracks if t.get('releasetype')}
    if str(release.get('type') or '').upper() in formats:
        score+=5;reasons.append('Release format is represented in the local discography')
    if release.get('official') is False:
        score -= 35; reasons.append('Provider marks this release unofficial')
    elif release.get('official') is True:
        score += 10; reasons.append('Provider marks this release official')
    score = max(0, min(100, score))
    badge='Recommended' if score>=80 else 'Potential' if score>=55 else 'Suspect' if score>=30 else 'Unmatched'
    return dict(score=score, badge=badge,
                evidence=reasons or ['Limited catalogue evidence; review the release'])
