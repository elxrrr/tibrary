"""One row per artist, including all of their confirmed catalogue identities."""
from .core import norm


def favourite_rows(favourites, local_artists, mappings):
    parent = {}
    def find(key):
        parent.setdefault(key, key)
        if parent[key] != key:
            parent[key] = find(parent[key])
        return parent[key]
    def join(a, b):
        parent[find(b)] = find(a)
    local = {name: ('name', norm(name)) for name in local_artists}
    for key in local.values(): find(key)
    for item in favourites:
        if item.get('name') and item.get('id') is not None:
            join(('name', norm(item['name'])), ('id', str(item['id'])))
    for item in mappings:
        if item['artist'] in local and item.get('tidal_id') is not None:
            join(local[item['artist']], ('id', str(item['tidal_id'])))
    groups = {}
    def group(key):
        return groups.setdefault(find(key), {'local': [], 'names': [], 'ids': set(), 'favourite': False})
    for name, key in local.items(): group(key)['local'].append(name)
    for item in favourites:
        if not item.get('name') or item.get('id') is None: continue
        g = group(('id', str(item['id'])))
        g['names'].append(item['name']); g['ids'].add(str(item['id'])); g['favourite'] = True
    for item in mappings:
        if item['artist'] in local and item.get('tidal_id') is not None:
            group(local[item['artist']])['ids'].add(str(item['tidal_id']))
    rows = []
    for g in groups.values():
        names = g['local'] or g['names']
        name = sorted(names, key=str.casefold)[0]
        category = ('In library' if g['local'] else 'Missing locally') if g['favourite'] else 'Local only'
        count = sum(len(local_artists[n]) for n in g['local'])
        ids = ', '.join(sorted(g['ids'])) or '—'
        rows.append((name, category, str(count) if g['local'] else '—', ids,
                     'Favourited & in library' if category == 'In library' else category))
    return sorted(rows, key=lambda r: r[0].casefold())
