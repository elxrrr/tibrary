"""Small connection checks: no catalogue traversal and no browser sign-in."""
from .credentials import CredentialError
from .tidal import CatalogueError
import time


def check_connections(credentials, account, api, progress=lambda message: None, details=False):
    messages = [];metrics={}
    def record(message,key=None,ok=None,started=None):
        messages.append(message)
        progress(message)
        if key:metrics[key]=dict(ok=bool(ok),latency_ms=round((time.monotonic()-started)*1000) if started else None,message=message)
    started=time.monotonic()
    try:
        credentials.get()
    except CredentialError as exc:
        record(str(exc),'credentials',False,started)
        result='\n'.join(messages)
        return dict(summary=result,metrics=metrics) if details else result
    record('Application credentials: configured.','credentials',True,started)
    started=time.monotonic()
    try:
        api.authenticate()
        record('Application credentials: connected.','catalogue',True,started)
    except (CredentialError, CatalogueError) as exc:
        record('Application credentials: ' + str(exc),'catalogue',False,started)
    started=time.monotonic()
    try:
        connected = bool(account.load())
    except (CredentialError, CatalogueError) as exc:
        connected = False
        record('Account session: ' + str(exc))
    if connected:
        started=time.monotonic()
        try:
            api.user_token = account.token
            api.get(api.BASE + 'userCollectionArtists/me/relationships/items?include=items')
            record('Favourites access: connected (first page checked).','account',True,started)
        except (CredentialError, CatalogueError) as exc:
            record('Favourites access: ' + str(exc),'account',False,started)
    else:
        record('Favourites: sign in under Settings → Connections.','account',False,started)
    started=time.monotonic()
    try:
        api.user_token = account.search_token if connected else None
        api.token = None
        # Only the search summary is needed; do not follow its relationships.
        api.get(api.url('searchResults', **{'filter[query]': 'TIDAL'}))
        record('Catalogue search: connected.','search',True,started)
    except (CredentialError, CatalogueError) as exc:
        record('Catalogue search: ' + str(exc),'search',False,started)
    finally:
        api.user_token = None
    result='\n'.join(messages)
    return dict(summary=result,metrics=metrics) if details else result
