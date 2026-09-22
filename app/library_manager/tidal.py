"""Official catalogue-only API boundary. No playback or download endpoints."""
import base64
import json
import os
import re
import time
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode, urlparse, parse_qsl, unquote, urlunparse
from urllib.request import Request, build_opener, HTTPRedirectHandler


class CatalogueError(Exception):
    def __init__(self, message, status=None, batch_fatal=False):
        super().__init__(message)
        self.status = status
        self.batch_fatal = batch_fatal


def error_detail(exc):
    """Use allowlisted diagnostic identifiers, never echoed response text or URLs."""
    try:
        payload = json.loads(exc.read(16384))
        errors = payload.get('errors', [])[:3]
        parts = []
        for error in errors:
            code = str(error.get('code', ''))
            parameter = str(error.get('source', {}).get('parameter', ''))
            if re.fullmatch(r'[A-Z_0-9.-]{1,64}', code):
                parts.append(f'code {code}')
            if parameter in {'countryCode', 'filter[query]', 'include', 'id', 'page[cursor]', 'scope'}:
                parts.append(f'parameter {parameter}')
        return '; '.join(parts)
    except Exception:
        return ''
    finally:
        exc.close()



def next_page_url(current, link):
    """Resolve TIDAL's API-relative links without trusting a new origin.

    TIDAL's Links schema uses /artists/... relative to the /v2 API base,
    whereas normal urljoin would resolve it against the website root.
    """
    if link is None:
        return None
    if isinstance(link, dict):
        if 'href' not in link:
            raise CatalogueError('TIDAL returned a next-page link without href; incomplete refresh discarded.')
        link = link['href']
    if not isinstance(link, str) or not link or any(ord(c) < 33 for c in link) or '\\' in link:
        raise CatalogueError('TIDAL returned an invalid next-page link; incomplete refresh discarded.')
    try:
        parsed = urlparse(link)
        previous = urlparse(current)
        if parsed.fragment or parsed.params:
            raise ValueError()
        if parsed.scheme or parsed.netloc:
            if (parsed.scheme and parsed.scheme != 'https') or parsed.netloc != 'openapi.tidal.com':
                raise ValueError()
        path = parsed.path
        if any(part in ('.', '..') for part in unquote(path).split('/')):
            raise ValueError()
        family = previous.path.split('/')[2]
        if not path:
            path = previous.path
        elif not path.startswith('/v2/'):
            # Only the current endpoint family can omit the API prefix.
            if path.lstrip('/').split('/')[0] != family:
                raise ValueError()
            path = '/v2/' + path.lstrip('/')
        if path.split('/')[2] != family:
            raise ValueError()
        query = parsed.query
        supplied = {key for key, _ in parse_qsl(query, keep_blank_values=True)}
        # Preserve context, not the old cursor. Keep opaque cursor encoding intact.
        carry = [(key, value) for key, value in parse_qsl(previous.query, keep_blank_values=True)
                 if key in {'countryCode', 'include', 'locale', 'explicitFilter', 'sort'} and key not in supplied]
        if carry:
            query += ('&' if query else '') + urlencode(carry)
        return urlunparse(('https', 'openapi.tidal.com', path, '', query, ''))
    except (ValueError, IndexError):
        raise CatalogueError('Rejected a next-page link outside the expected HTTPS TIDAL API endpoint; incomplete refresh discarded.') from None


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


class RequestPacer:
    """Share a modest request spacing across jobs; still honor server backoff."""
    def __init__(self, interval=.35):
        import threading
        self.base_interval = interval
        self.interval = interval
        self.next_at = 0
        self.backoff_until = 0
        self.rate_limits=[]
        self.paused_until=0
        self.lock = threading.Lock()

    def configure(self, interval):
        with self.lock:
            self.base_interval=max(.1,min(5.0,float(interval)))
            if not self.backoff_until:self.interval=self.base_interval

    def record_429(self, delay=2.0):
        with self.lock:
            now=time.monotonic()
            self.rate_limits=[stamp for stamp in self.rate_limits if now-stamp<180]
            self.rate_limits.append(now)
            self.interval=max(self.interval,1.0)
            delay=max(30.0,delay)
            self.backoff_until=max(self.backoff_until,now+delay)
            if len(self.rate_limits)>=3:
                self.backoff_until=max(self.backoff_until,now+180)
                self.paused_until=self.backoff_until
                raise CatalogueError('Repeated rate limiting · linking paused. Wait a few minutes before resuming; completed results are saved.',status=429,batch_fatal=True)
            return delay

    def wait(self, cancelled):
        with self.lock:
            if time.monotonic()<self.paused_until:
                raise CatalogueError('Rate-limit cooldown is still active. Wait a few minutes before resuming; saved results retained.',status=429,batch_fatal=True)
            if self.backoff_until and time.monotonic() > self.backoff_until:
                self.interval = self.base_interval
                self.backoff_until = 0
            now = time.monotonic()
            wait_target = max(now, self.next_at, self.backoff_until)
            self.next_at = wait_target + self.interval
        while time.monotonic() < wait_target:
            if cancelled():
                raise CatalogueError('Cancelled while waiting to send a catalogue request.')
            time.sleep(min(.05, max(0, wait_target - time.monotonic())))


class Tidal:
    BASE = 'https://openapi.tidal.com/v2/'

    def __init__(self, market='GB', cancelled=lambda: False, transport=None, credentials=None, progress=lambda s: None, user_token=None, search_user_token=None, pacer=None, settings=None):
        from .client_settings import normalized
        self.settings=normalized(settings)
        self.pacer = pacer or (RequestPacer() if transport is None else None)
        self.search_user_token = search_user_token
        self.user_token = user_token
        self.credentials = credentials
        self.progress = progress
        self.market = market
        self.cancelled = cancelled
        self.transport = transport or build_opener(NoRedirect()).open
        self.token = None
        self.expires = 0

    def check(self):
        if self.cancelled():
            raise CatalogueError('Cancelled; last complete catalogue retained.')

    def authenticate(self):
        self.check()
        self.progress('Authentication · reading configured credentials (your OS may ask to unlock the keychain)')
        client, secret = self.credentials() if self.credentials else (os.getenv('TIDAL_CLIENT_ID'), os.getenv('TIDAL_CLIENT_SECRET'))
        if not client or not secret:
            raise CatalogueError('Open Settings → Connections to enter a client ID and secret. Demo mode needs no credentials.')
        header = base64.b64encode(f'{client}:{secret}'.encode()).decode()
        request = Request('https://auth.tidal.com/v1/oauth2/token',
                          data=urlencode({'grant_type': 'client_credentials'}).encode(),
                          headers={'Authorization': f'Basic {header}', 'Content-Type': 'application/x-www-form-urlencoded'})
        timeout=self.settings['request_timeout_sec']
        self.progress(f'Authentication · requesting an access token from TIDAL ({timeout}-second timeout)')
        try:
            with self.transport(request, timeout=timeout) as response:
                token = json.load(response)
            self.token = token['access_token']
            if not isinstance(self.token, str) or not self.token:
                raise ValueError('Missing access token')
            self.expires = time.monotonic() + int(token.get('expires_in', 300)) - self.settings['token_refresh_margin_sec']
        except HTTPError as exc:
            exc.close()
            detail = 'Check the client ID and secret in Settings → Connections.' if exc.code in (400, 401, 403) else 'Try again later.'
            raise CatalogueError(f'TIDAL authentication failed (HTTP {exc.code}). {detail}') from None
        except (URLError, TimeoutError):
            raise CatalogueError(f'Could not reach TIDAL authentication. Check your connection and try again ({timeout}-second request timeout).', batch_fatal=True) from None
        except Exception:
            raise CatalogueError('TIDAL authentication returned an invalid response. Try again later.') from None
        self.check()
        self.progress('Authentication succeeded · access token held in memory')

    def get(self, url):
        parsed = urlparse(url)
        if parsed.scheme != 'https' or parsed.netloc != 'openapi.tidal.com' or not parsed.path.startswith('/v2/'):
            raise CatalogueError('Rejected an unexpected catalogue pagination URL.')
        attempts=self.settings['request_attempts'];timeout=self.settings['request_timeout_sec']
        for attempt in range(attempts):
            self.check()
            fresh_client_token = False
            if self.user_token:
                self.token = self.user_token(False)
            elif not self.token or time.monotonic() >= self.expires:
                self.authenticate()
                fresh_client_token = True
            try:
                if self.pacer:
                    self.pacer.wait(self.cancelled)
                self.progress(f'Catalogue request · {parsed.path.split("/")[2]} · attempt {attempt + 1}/{attempts} · waiting for TIDAL ({timeout}-second timeout)')
                request = Request(url, headers={'Authorization': f'Bearer {self.token}', 'Accept': 'application/vnd.api+json'})
                with self.transport(request, timeout=timeout) as response:
                    return json.load(response)
            except HTTPError as exc:
                diagnostic = error_detail(exc)
                if exc.code == 401 and attempt == 0 and not fresh_client_token:
                    self.progress('Access token rejected · retrying authorization once (expiry is not confirmed)')
                    if self.user_token:
                        self.user_token(True)
                    self.token = None
                    continue
                if exc.code not in (429, 500, 502, 503, 504) or attempt == attempts-1:
                    hints = {400: 'TIDAL rejected the request format or parameters; changing the client secret will not fix this.',
                             401: ('Account authorization was rejected. Check the required scopes and reconnect your account.' if self.user_token else
                                   'Client authentication succeeded, but this endpoint denied app access. For search, connect your account with search.read permission; replacing the secret will not grant endpoint access.'),
                             403: 'This application or account lacks permission for this endpoint. Check its enabled scopes.',
                             404: 'This catalogue resource is unavailable or no longer exists.',
                             429: 'Rate limit reached. Try again later.'}
                    detail = f' ({diagnostic})' if diagnostic else ''
                    family = parsed.path.split('/')[2]
                    raise CatalogueError(f'TIDAL {family} request failed (HTTP {exc.code}){detail}. '
                                         + hints.get(exc.code, 'TIDAL is temporarily unavailable. Try again later.')
                                         + ' Cached data retained.', status=exc.code) from None
                try:
                    delay = max(0, float(exc.headers.get('Retry-After', 2 ** attempt)))
                except ValueError:
                    delay = 2 ** attempt
                if exc.code == 429 and self.pacer:
                    delay=self.pacer.record_429(delay)
                if delay > 30:
                    raise CatalogueError('TIDAL requested a longer pause. Try refreshing later; cached data retained.', status=429) from None
                self.progress(f'TIDAL returned HTTP {exc.code} · retrying in {delay:g} seconds')
                until = time.monotonic() + delay
                while time.monotonic() < until:
                    self.check()
                    time.sleep(min(.1, max(0, until - time.monotonic())))
            except (URLError, TimeoutError, ValueError):
                raise CatalogueError('Catalogue connection or response failed; cached data retained.', batch_fatal=True) from None

    def url(self, path, **params):
        return self.BASE + path + '?' + urlencode(dict(countryCode=self.market, **params))

    def entities(self, path, kind, include, initial_url=None, skip_missing=False, include_videos=False, included_entities=None):
        url = initial_url or self.url(path, include=include)
        seen, result = set(), {}
        unavailable = set()
        while url:
            parsed_page = urlparse(url)
            page_key = (parsed_page.scheme, parsed_page.netloc, parsed_page.path, tuple(sorted(parse_qsl(parsed_page.query, keep_blank_values=True))))
            if page_key in seen:
                raise CatalogueError('Repeated catalogue page; incomplete refresh discarded.')
            seen.add(page_key)
            self.progress(f'Fetching {kind} · page {len(seen)} · {len(result)} received so far')
            payload = self.get(url)
            included = {(r['type'], r['id']): r for r in payload.get('included', [])}
            if included_entities is not None:included_entities.update(included)
            data = payload.get('data', [])
            if not isinstance(data, list):
                data = [data]
            for ref in data:
                if ref['type'] != kind:
                    if include_videos and ref['type']=='videos':
                        result['video:'+ref['id']]=dict(ref)
                    continue
                if ref['id'] in unavailable:
                    continue
                entity = ref if 'attributes' in ref else included.get((kind, ref['id']))
                if entity is None:
                    try:
                        entity = self.get(self.url(f'{kind}/{quote(ref["id"], safe="")}'))['data']
                    except CatalogueError as exc:
                        if not skip_missing or exc.status != 404:
                            raise
                        unavailable.add(ref['id'])
                        self.progress(f'Skipping unavailable favourite artist ID {ref["id"]} · TIDAL returned 404 · continuing with the collection')
                        continue
                result[entity['id']] = dict(entity, relationship_meta=ref.get('meta', {}))
            link = payload.get('links', {}).get('next')
            url = next_page_url(url, link)
        self.progress(f'Finished {kind} · {len(result)} items across {len(seen)} page(s)' + (f' · {len(unavailable)} unavailable favourites skipped' if unavailable else ''))
        return list(result.values())

    def search(self, name):
        if self.search_user_token and self.search_user_token(False):
            self.progress('Global search · using your account authorization with search.read permission')
            return Tidal(self.market, self.cancelled, self.transport, progress=self.progress, user_token=self.search_user_token, pacer=self.pacer,settings=self.settings).search(name)
        self.progress(f'Searching TIDAL artists for “{name}” in {self.market}')
        # The collection endpoint returns opaque search-result IDs. A raw query
        # is NOT a valid ID for /searchResults/{id}/relationships/artists.
        payload = self.get(self.url('searchResults', **{'filter[query]': name}))
        data = payload.get('data')
        if not isinstance(data, list):
            raise CatalogueError('TIDAL returned an invalid search response; cached data retained.')
        artists = {}
        for result in data:
            ident = result.get('id')
            if not isinstance(ident, str) or not ident:
                raise CatalogueError('TIDAL did not return a search-result ID; cached data retained.')
            for artist in self.entities(f'searchResults/{quote(ident, safe="")}/relationships/artists', 'artists', 'artists'):
                artists[artist['id']] = dict(id=artist['id'], name=artist['attributes']['name'])
        return list(artists.values())

    def search_albums(self, query):
        if self.search_user_token and self.search_user_token(False):
            return Tidal(self.market, self.cancelled, self.transport, progress=self.progress, user_token=self.search_user_token, pacer=self.pacer,settings=self.settings).search_albums(query)
        self.progress(f'Searching TIDAL albums for “{query}” in {self.market}')
        payload = self.get(self.url('searchResults', **{'filter[query]': query}))
        data = payload.get('data')
        if isinstance(data, dict): data = [data]
        if not isinstance(data, list):
            return []
        albums = {}
        for result in data:
            ident = result.get('id')
            if not isinstance(ident, str) or not ident:
                continue
            try:
                for item in self.entities(f'searchResults/{quote(ident, safe="")}/relationships/albums', 'albums', 'albums'):
                    attr = item.get('attributes', {})
                    albums[item['id']] = dict(
                        id=item['id'],
                        title=title(attr),
                        date=attr.get('releaseDate', ''),
                        track_count=attr.get('numberOfItems'),
                        available=None,
                        copyright=attr.get('copyright')
                    )
            except CatalogueError as exc:
                if exc.status not in (404, 410): raise
        return list(albums.values())

    def search_tracks(self, query):
        if self.search_user_token and self.search_user_token(False):
            return Tidal(self.market, self.cancelled, self.transport, progress=self.progress, user_token=self.search_user_token, pacer=self.pacer,settings=self.settings).search_tracks(query)
        self.progress(f'Searching TIDAL tracks for “{query}” in {self.market}')
        payload = self.get(self.url('searchResults', **{'filter[query]': query}))
        data = payload.get('data')
        if isinstance(data, dict): data = [data]
        if not isinstance(data, list):
            return []
        tracks = []
        for result in data:
            ident = result.get('id')
            if not isinstance(ident, str) or not ident:
                continue
            try:
                for trk in self.entities(f'searchResults/{quote(ident, safe="")}/relationships/tracks', 'tracks', 'albums'):
                    attr = trk.get('attributes', {})
                    album_refs = trk.get('relationships', {}).get('albums', {}).get('data', [])
                    album_ids = [str(a['id']) for a in album_refs if 'id' in a]
                    tracks.append(dict(id=trk['id'], title=title(attr), isrc=attr.get('isrc'), album_ids=album_ids))
            except CatalogueError as exc:
                if exc.status not in (404, 410): raise
        return tracks

    def favourite_artists(self):
        if not self.user_token:
            raise CatalogueError('Connect your TIDAL account to read favourited artists. Client credentials alone cannot access your collection.')
        self.progress('Reading your favourited artists · read-only collection access')
        url = self.BASE + 'userCollectionArtists/me/relationships/items?' + urlencode({'include': 'items'})
        return [dict(id=a['id'], name=a['attributes']['name']) for a in
                self.entities('', 'artists', 'items', initial_url=url, skip_missing=True)]

    def add_favourite_artist(self, ident):
        if not self.user_token:
            raise CatalogueError('Connect your TIDAL account to add favourite artists.')
        url = self.BASE + 'userCollectionArtists/me/relationships/items'
        for attempt in range(2):
            self.check()
            token = self.user_token(False)
            try:
                if self.pacer: self.pacer.wait(self.cancelled)
                body = json.dumps({"data": [{"id": str(ident), "type": "artists"}]}).encode()
                request = Request(url, data=body, method='POST', headers={
                    'Authorization': f'Bearer {token}',
                    'Content-Type': 'application/vnd.api+json',
                    'Accept': 'application/vnd.api+json'
                })
                with self.transport(request, timeout=10) as response:
                    return response.read()
            except HTTPError as exc:
                if exc.code == 401 and attempt == 0:
                    self.user_token(True)
                    continue
                raise CatalogueError(f'TIDAL favourite update failed (HTTP {exc.code}).') from None
            except Exception as exc:
                raise CatalogueError(f'Could not update TIDAL favourite: {exc}') from exc

    def artist(self, ident, progress=lambda s: None, detailed=False):
        if not str(ident).isdigit():
            raise CatalogueError('Enter a numeric TIDAL artist ID.')
        self.progress(f'Loading release summaries for artist {ident} · track lists are fetched only on request')
        artist = self.get(self.url(f'artists/{ident}'))['data']
        releases = []
        albums = self.entities(f'artists/{ident}/relationships/albums', 'albums', 'albums')
        for album in albums:
            self.check()
            a = album['attributes']
            availability = a.get('availability')
            is_available = bool(set(availability) & {'STREAM', 'DJ'}) if availability is not None else None
            release = dict(id=album['id'], artist=artist['attributes']['name'], title=title(a),
                           date=a.get('releaseDate', ''), type=a.get('albumType', a.get('type')),
                           available=is_available,
                           link_checked_at=time.time() if is_available is True else None,
                           quality=', '.join(a.get('mediaTags', [])), track_count=a.get('numberOfItems'), explicit=a.get('explicit'),
                           copyright=a.get('copyright'),label=a.get('recordLabel') or a.get('label'),
                           tracks=[], tracks_loaded=False)
            releases.append(self.release_details(release) if detailed else release)
        progress(f'Release catalogue ready · {len(releases)} releases · ' + ('track details loaded' if detailed else 'no track-list requests'))
        return dict(id=str(ident), name=artist['attributes']['name'], releases=releases)

    def release_availability(self, release, force=False):
        if not force and release.get('available') is not None:
            return release
        try:
            payload = self.get(self.url(f'albums/{release["id"]}', include='usageRules'))
        except CatalogueError as exc:
            if not force or exc.status not in (404, 410): raise
            return dict(release, available=False, link_checked_at=time.time())
        attributes = payload['data'].get('attributes', {})
        availability = attributes.get('availability')
        available = bool(set(availability) & {'STREAM', 'DJ'}) if availability is not None else None
        usage = [r['attributes'] for r in payload.get('included', []) if r['type'] == 'usageRules']
        if usage:
            allowed = set(usage[0].get('subscription', [])) | set(usage[0].get('free', [])) | set(usage[0].get('paid', []))
            available = bool(allowed & {'STREAM', 'DJ'})
        result = dict(release, available=available)
        if force:
            result.update(link_checked_at=time.time(), date=attributes.get('releaseDate',release.get('date','')),
                          explicit=attributes.get('explicit', release.get('explicit')),
                          track_count=attributes.get('numberOfItems', release.get('track_count')),
                          copyright=attributes.get('copyright', release.get('copyright')))
        return result

    def release_details(self, release):
        items = self.entities(f'albums/{release["id"]}/relationships/items', 'tracks', 'items',include_videos=True)
        tracks=[item for item in items if item['type']=='tracks']
        video_count=sum(item['type']=='videos' for item in items)
        expected = release.get('item_count',release.get('track_count'))
        if expected is not None and len(items) != int(expected):
            raise CatalogueError('Album item list is incomplete; saved summary retained.')
        release = self.release_availability(release)
        return dict(release, tracks_loaded=True, item_count=len(items), video_count=video_count, track_count=len(tracks), tracks=[
            dict(id=t['id'], title=title(t['attributes']), isrc=t['attributes'].get('isrc'),
                 track_number=t.get('relationship_meta', {}).get('trackNumber', t['attributes'].get('trackNumber')),
                 disc_number=t.get('relationship_meta', {}).get('volumeNumber', t['attributes'].get('volumeNumber')),
                 copyright=t['attributes'].get('copyright'),bpm=t['attributes'].get('bpm'),key=t['attributes'].get('key'),key_scale=t['attributes'].get('keyScale'),
                 duration=duration(t['attributes'].get('duration', ''))) for t in tracks])

    def recording_releases(self, isrc=None, track_id=None):
        included={}
        tracks = self.entities('', 'tracks', 'albums', initial_url=self.url('tracks', include='albums', **({'filter[id]': track_id} if track_id else {'filter[isrc]': isrc})),included_entities=included)
        ids = {str(ref['id']) for track in tracks for ref in track.get('relationships', {}).get('albums', {}).get('data', [])}
        result = []
        for ident in sorted(ids, reverse=True):
            try:
                attributes = included.get(('albums',ident),{}).get('attributes')
                if not attributes:attributes = self.get(self.url(f'albums/{ident}'))['data']['attributes']
                result.append(dict(id=ident, title=title(attributes), artist='', date=attributes.get('releaseDate',''),
                                   track_count=attributes.get('numberOfItems'), available=None,
                                   copyright=attributes.get('copyright')))
            except CatalogueError as exc:
                if exc.status not in (404, 410): raise
        return result

    def album_tag_details(self, release):
        detailed = self.release_details(release)
        payload = self.get(self.url(f'albums/{release["id"]}', include='artists,genres,coverArt'))
        attributes = payload['data']['attributes']
        relation = payload['data'].get('relationships', {}).get('artists', {})
        included = {str(r['id']): r['attributes']['name'] for r in payload.get('included', []) if r['type'] == 'artists'}
        refs = relation.get('data', [])
        credits = [included[str(r['id'])] for r in refs if str(r['id']) in included]
        complete = bool(refs) and len(credits) == len(refs) and not relation.get('links', {}).get('next')
        return dict(detailed, title=title(attributes), date=attributes.get('releaseDate', ''),
                    artist=', '.join(credits) or detailed.get('artist') or 'Unknown artist',
                    type=attributes.get('albumType') or attributes.get('type') or detailed.get('type'),
                    disc_count=attributes.get('numberOfVolumes'), album_artists=credits, album_artist_ids=[str(r['id']) for r in refs],
                    cover_files=[f for r in payload.get('included',[]) if r['type']=='artworks' for f in r['attributes'].get('files',[])],
                    genres=[r['attributes']['genreName'] for r in payload.get('included',[]) if r['type']=='genres'],
                    copyright=attributes.get('copyright'),barcode=attributes.get('barcodeId'),
                    label=attributes.get('recordLabel') or attributes.get('label'),
                    credits_complete=complete, tag_credits_checked=True)

    def track_tag_details(self, track):
        payload=self.get(self.url(f'tracks/{track["id"]}',include='genres,artists,credits'))
        attrs=payload['data']['attributes'];included=payload.get('included',[])
        return dict(track,title=title(attrs),isrc=attrs.get('isrc'),copyright=attrs.get('copyright'),
                    bpm=attrs.get('bpm'),key=attrs.get('key'),key_scale=attrs.get('keyScale'),
                    initialkey=attrs.get('initialKey'),tkey=attrs.get('TKEY'),musical_key=attrs.get('musicalKey'),
                    artists=[r['attributes']['name'] for r in included if r['type']=='artists'],
                    genres=[r['attributes']['genreName'] for r in included if r['type']=='genres'],
                    credits=[r['attributes'] for r in included if r['type']=='credits'])


def title(attributes):
    value = attributes['title']
    version = attributes.get('version')
    return f'{value} ({version})' if version and version.casefold() not in value.casefold() else value


def duration(value):
    match = re.fullmatch(r'PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?', value)
    return sum(float(v or 0) * m for v, m in zip(match.groups(), (3600, 60, 1))) if match else None
