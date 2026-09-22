"""Official browser authorization for read-only user collections (PKCE)."""
import base64
import hashlib
import hmac
import json
import secrets
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import parse_qs, urlencode, urlparse
from urllib.request import Request, build_opener
from urllib.error import HTTPError, URLError

from .credentials import CredentialError
from .tidal import CatalogueError, NoRedirect

REDIRECT = 'http://127.0.0.1:8765/callback'
SCOPE = 'collection.read search.read'


class Account:
    def __init__(self, credentials, transport=None, settings=None):
        self.credentials = credentials
        self.transport = transport or build_opener(NoRedirect()).open
        self.profile = None
        self.client_key = None
        self.settings=settings or {}

    def config(self):
        from .client_settings import normalized
        return normalized(self.settings() if callable(self.settings) else self.settings)

    def key(self):
        client, _ = self.credentials.get()
        return 'collection-' + hashlib.sha256(client.encode()).hexdigest()

    def load(self):
        key = self.key()
        if key != self.client_key:
            self.profile = None
            self.client_key = key
        if self.profile is None:
            try:
                saved = self.credentials.backend_factory().get_password(self.credentials.SERVICE, key)
                if saved:
                    profile = json.loads(saved)
                    if not isinstance(profile.get('access_token'), str) or not profile.get('cache_id'):
                        raise ValueError('Invalid account session')
                    self.profile = profile
            except Exception:
                raise CredentialError('Could not read the saved TIDAL account session. Unlock Keychain or reconnect your account.') from None
        return self.profile

    def logged_in(self):
        try:
            profile = self.load()
            return bool(profile and isinstance(profile.get('access_token'), str) and profile.get('access_token'))
        except Exception:
            return False

    def exchange(self, data):
        client, secret = self.credentials.get()
        authorization = base64.b64encode(f'{client}:{secret}'.encode()).decode()
        request = Request('https://auth.tidal.com/v1/oauth2/token', data=urlencode(dict(data, client_id=client)).encode(),
                          headers={'Authorization': f'Basic {authorization}', 'Content-Type': 'application/x-www-form-urlencoded'})
        try:
            config=self.config()
            with self.transport(request, timeout=config['request_timeout_sec']) as response:
                token = json.load(response)
            if not isinstance(token.get('access_token'), str) or not token['access_token']:
                raise ValueError('Missing token')
            if token.get('scope') and 'collection.read' not in token['scope'].split():
                raise CatalogueError('Collection read permission was not granted. Enable collection.read for your developer application and reconnect.')
            token['expires_at'] = time.time() + int(token.get('expires_in', 300)) - config['token_refresh_margin_sec']
            return token
        except HTTPError as exc:
            exc.close()
            raise CatalogueError(f'TIDAL account authorization failed (HTTP {exc.code}). Check the registered redirect URI and collection.read permission, then reconnect.', status=exc.code) from None
        except (URLError, TimeoutError):
            raise CatalogueError('Account authorization could not reach TIDAL. Check your connection and try again.', batch_fatal=True) from None
        except (ValueError, KeyError, TypeError):
            raise CatalogueError('TIDAL returned an invalid account authorization response.') from None

    def token(self, force=False):
        profile = self.load()
        if not profile:
            raise CatalogueError('Connect your TIDAL account to read favourited artists.')
        if force or time.time() >= profile['expires_at']:
            if not profile.get('refresh_token'):
                raise CatalogueError('Account session expired. Reconnect your TIDAL account.')
            fresh = self.exchange({'grant_type': 'refresh_token', 'refresh_token': profile['refresh_token']})
            profile = dict(profile, **fresh)
            self.persist(profile)
        return profile['access_token']

    def search_token(self, force=False):
        profile = self.load()
        if not profile:
            return None
        if 'search.read' not in profile.get('scope', '').split():
            raise CatalogueError('Your saved account sign-in grants collection access but not search.read. Enable search.read in your TIDAL developer app, then reconnect the account. Favourites still work.')
        return self.token(force)

    def persist(self, profile):
        if profile.get('remember'):
            try:
                self.credentials.backend_factory().set_password(self.credentials.SERVICE, self.key(), json.dumps(profile))
            except Exception:
                raise CredentialError('Could not securely save the account session. Unlock Keychain or connect with session-only storage.') from None
        self.profile = profile
        self.client_key = self.key()

    def connect(self, open_browser, cancelled, progress, remember=True):
        client, _ = self.credentials.get()
        verifier = secrets.token_urlsafe(48)
        state = secrets.token_urlsafe(32)
        challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b'=').decode()
        result = {}
        class Callback(BaseHTTPRequestHandler):
            def setup(self):
                super().setup()
                self.connection.settimeout(2)
            def log_message(self, *args): pass  # OAuth callback contains a code; never log it.
            def do_GET(self):
                parsed = urlparse(self.path)
                args = parse_qs(parsed.query)
                valid = parsed.path == '/callback' and hmac.compare_digest(args.get('state', [''])[0], state)
                if valid:
                    result['code'] = args.get('code', [None])[0]
                    result['denied'] = bool(args.get('error')) or not result['code']
                body = b'You can return to TIDAL Library Manager.' if valid else b'Invalid authorization callback.'
                self.send_response(200 if valid else 400)
                self.send_header('Content-Type', 'text/plain; charset=utf-8')
                self.send_header('Cache-Control', 'no-store')
                self.send_header('Content-Length', str(len(body))); self.end_headers()
                self.wfile.write(body)
        try:
            server = HTTPServer(('127.0.0.1', 8765), Callback)
        except OSError:
            raise CatalogueError('The account sign-in port (8765) is in use. Close the other sign-in attempt and retry.') from None
        with server:
            server.timeout = .5
            url = 'https://login.tidal.com/authorize?' + urlencode(dict(client_id=client, response_type='code',
                   redirect_uri=REDIRECT, scope=SCOPE, state=state, code_challenge=challenge, code_challenge_method='S256'))
            timeout=self.config()['sign_in_timeout_sec']
            progress(f'Opening TIDAL in your browser · sign in and approve read-only collection and search access · waiting up to {timeout//60} minutes')
            open_browser(url)
            until = time.monotonic() + timeout
            while not result and time.monotonic() < until:
                if cancelled():
                    raise CatalogueError('Account sign-in cancelled. Existing saved session unchanged.')
                server.handle_request()
            if not result:
                raise CatalogueError('Account sign-in timed out. Check the registered redirect URI, then try again.')
            if result['denied']:
                raise CatalogueError('Account access was not granted. Your previous session is unchanged.')
        if cancelled():
            raise CatalogueError('Account sign-in cancelled. Existing saved session unchanged.')
        progress('Exchanging the browser authorization for a read-only account session')
        token = self.exchange(dict(grant_type='authorization_code', code=result['code'], redirect_uri=REDIRECT, code_verifier=verifier))
        if cancelled():
            raise CatalogueError('Account sign-in cancelled. Existing saved session unchanged.')
        if not token.get('scope'):
            token['scope'] = SCOPE
        token.update(cache_id=secrets.token_hex(16), remember=remember)
        self.persist(token)
        return ('TIDAL account connected. Favourited artists will be checked first.' +
                (' Global search permission was not granted; enable search.read and reconnect for global search.' if 'search.read' not in token['scope'].split() else ' Global search permission granted.'))

    def disconnect(self):
        key = self.key()
        try:
            backend = self.credentials.backend_factory()
            if backend.get_password(self.credentials.SERVICE, key) is not None:
                backend.delete_password(self.credentials.SERVICE, key)
        except Exception:
            raise CredentialError('Could not remove the saved account session. Unlock Keychain and retry.') from None
        self.profile = None
        return 'Account session removed from this app. Revoke access in TIDAL to revoke the grant itself.'
