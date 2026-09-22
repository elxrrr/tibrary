import io
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError

from library_manager.credentials import Credentials, CredentialError
from library_manager.core import Store, scan
from library_manager.tidal import Tidal, CatalogueError


class MemoryKeychain:
    def __init__(self): self.values = {}
    def get_password(self, service, account): return self.values.get((service, account))
    def set_password(self, service, account, value): self.values[service, account] = value
    def delete_password(self, service, account): del self.values[service, account]


class ConnectionTests(unittest.TestCase):
    def setUp(self):
        self.backend = MemoryKeychain()
        self.credentials = Credentials(lambda: self.backend)

    def test_saved_pair_survives_new_instance_and_forget(self):
        self.credentials.save('example-client', 'example-secret')
        reopened = Credentials(lambda: self.backend)
        self.assertEqual(reopened.get(), ('example-client', 'example-secret'))
        reopened.forget()
        self.assertEqual(self.backend.values, {})
        self.assertIsNone(reopened.session)

    def test_session_only_does_not_write_or_replace_saved_pair(self):
        self.credentials.save('old-client', 'old-secret')
        self.credentials.save('new-client', 'new-secret', remember=False)
        self.assertEqual(self.credentials.get(), ('new-client', 'new-secret'))
        self.assertEqual(Credentials(lambda: self.backend).get(), ('old-client', 'old-secret'))

    def test_failed_save_is_safe_and_preserves_previous_configuration(self):
        self.credentials.save('previous-client', 'previous-secret', remember=False)
        self.backend.set_password = lambda *args: (_ for _ in ()).throw(RuntimeError('sensitive-secret'))
        with self.assertRaises(CredentialError) as raised:
            self.credentials.save('new-client', 'sensitive-secret')
        self.assertNotIn('sensitive-secret', str(raised.exception))
        self.assertEqual(self.credentials.get(), ('previous-client', 'previous-secret'))

    def test_credential_precedence_and_log_redaction(self):
        with patch.dict(os.environ, {'TIDAL_CLIENT_ID':'env-client', 'TIDAL_CLIENT_SECRET':'env-secret'}):
            self.assertEqual(self.credentials.get(), ('env-client', 'env-secret'))
            self.credentials.save('ui-client', 'ui-secret', remember=False)
            self.assertEqual(self.credentials.get(), ('ui-client', 'ui-secret'))
            self.assertEqual(self.credentials.redact('ui-client ui-secret env-client env-secret'), '[redacted] [redacted] [redacted] [redacted]')

    def test_missing_credentials_gives_actionable_message(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(CredentialError, 'Open Settings → Connections'):
                self.credentials.get()

    def test_authentication_uses_in_app_credentials_and_never_logs_secrets(self):
        self.credentials.save('ui-client', 'ui-secret', remember=False)
        events, requests = [], []
        def transport(request, **kwargs):
            requests.append(request)
            return io.BytesIO(json.dumps({'access_token':'private-token', 'expires_in':300}).encode())
        api = Tidal(credentials=self.credentials.get, progress=events.append, transport=transport)
        api.authenticate()
        self.assertIn('Authentication succeeded', events[-1])
        self.assertEqual(api.token, 'private-token')
        self.assertIn('Basic ', requests[0].get_header('Authorization'))
        log = '\n'.join(events)
        for secret in ('ui-client','ui-secret','private-token'):
            self.assertNotIn(secret, log)

    def test_auth_failure_explains_http_status_without_response_body(self):
        self.credentials.save('client', 'secret', remember=False)
        def transport(request, **kwargs):
            raise HTTPError(request.full_url, 401, 'secret', {}, io.BytesIO(b'secret'))
        with self.assertRaises(CatalogueError) as raised:
            Tidal(credentials=self.credentials.get, transport=transport).authenticate()
        self.assertIn('HTTP 401', str(raised.exception))
        self.assertIn('Settings → Connections', str(raised.exception))
        self.assertNotIn('b\'secret', str(raised.exception))

    def test_retry_reports_delay_and_page_progress(self):
        events = []
        responses = [{'data':[{'id':'opaque-result'}]}, HTTPError('https://openapi.tidal.com/v2/a',429,'rate limited',{'Retry-After':'0'},None), {'data':[]}]
        def transport(request, **kwargs):
            result = responses.pop(0)
            if isinstance(result, Exception): raise result
            return io.BytesIO(json.dumps(result).encode())
        api = Tidal(transport=transport, progress=events.append)
        api.token = 'never-log-this'; api.expires = float('inf')
        api.search('Example')
        log = '\n'.join(events)
        self.assertIn('page 1', log)
        self.assertIn('retrying in 0 seconds', log)
        self.assertIn('Finished artists', log)
        self.assertNotIn(api.token, log)

    def test_scan_explains_stages_and_final_counts(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / 'music'; root.mkdir(); (root / 'song.flac').write_bytes(b'test')
            store = Store(Path(folder) / 'catalogue.sqlite3'); events=[]
            scan(store, root, reader=lambda path: {'artist':'A','album':'B','title':'C'}, progress=events.append)
            log = '\n'.join(events)
            for text in ('Opening library','saved file inventory','Reading audio tags','updating the library index','Scan complete'):
                self.assertIn(text, log)
