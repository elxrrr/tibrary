import base64
import hashlib
import io
import json
import time
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlparse, parse_qs, urlencode

from library_manager.account import Account, REDIRECT
from library_manager.credentials import Credentials
from library_manager.tidal import Tidal, CatalogueError
from test_connection import MemoryKeychain


class FavouritesTests(unittest.TestCase):
    def setUp(self):
        self.credentials = Credentials(lambda: self.backend)
        self.backend = MemoryKeychain()
        self.credentials.save('test-client', 'test-secret', remember=False)

    def transport(self, responses, calls):
        def send(request, **kwargs):
            calls.append(request)
            response = responses.pop(0)
            if isinstance(response, Exception): raise response
            return io.BytesIO(json.dumps(response).encode())
        return send

    def test_search_uses_query_then_returned_opaque_id(self):
        calls = []
        api = Tidal(transport=self.transport([
            {'data':[{'id':'opaque/19', 'type':'searchResults'}]},
            {'data':[{'id':'12','type':'artists','attributes':{'name':'19Clouds'}}]}], calls))
        api.token='test'; api.expires=float('inf')
        self.assertEqual(api.search('19Clouds')[0]['id'], '12')
        self.assertEqual(urlparse(calls[0].full_url).path, '/v2/searchResults')
        self.assertEqual(parse_qs(urlparse(calls[0].full_url).query)['filter[query]'], ['19Clouds'])
        self.assertEqual(urlparse(calls[1].full_url).path, '/v2/searchResults/opaque%2F19/relationships/artists')

    def test_empty_search_is_empty_without_relationship_request(self):
        calls=[]; api=Tidal(transport=self.transport([{'data':[]}],calls))
        api.token='test'; api.expires=float('inf')
        self.assertEqual(api.search('missing'), []); self.assertEqual(len(calls),1)

    def test_400_is_not_retried_and_diagnostics_do_not_echo_secrets(self):
        calls=[]
        body = {'errors':[{'code':'INVALID_ID','detail':'secret-token', 'source':{'parameter':'id'}}]}
        error=HTTPError('https://openapi.tidal.com/v2/searchResults',400,'bad',{},io.BytesIO(json.dumps(body).encode()))
        api=Tidal(transport=self.transport([error],calls)); api.token='secret-token'; api.expires=float('inf')
        with self.assertRaises(CatalogueError) as caught: api.search('19Clouds')
        self.assertEqual(len(calls),1); self.assertEqual(caught.exception.status,400)
        self.assertIn('INVALID_ID', str(caught.exception)); self.assertNotIn('secret-token', str(caught.exception))
        self.assertIn('changing the client secret will not fix', str(caught.exception))

    def test_favourites_paginate_with_user_token(self):
        calls=[]
        responses=[{'data':[{'id':'1','type':'artists'}], 'included':[{'id':'1','type':'artists','attributes':{'name':'A'}}], 'links':{'next':'?page%5Bcursor%5D=two&include=items'}},
                   {'data':[{'id':'2','type':'artists','attributes':{'name':'B'}}]}]
        api=Tidal(transport=self.transport(responses,calls), user_token=lambda force: 'user-token')
        self.assertEqual([a['id'] for a in api.favourite_artists()], ['1','2'])
        self.assertTrue(all(r.get_header('Authorization') == 'Bearer user-token' for r in calls))
        self.assertIn('/userCollectionArtists/me/relationships/items?', calls[0].full_url)
        self.assertNotIn('countryCode', calls[0].full_url)

    def test_favourites_reject_client_only_token(self):
        with self.assertRaisesRegex(CatalogueError, 'Connect your TIDAL account'):
            Tidal().favourite_artists()

    def test_refresh_and_saved_session_are_bound_to_client(self):
        calls=[]
        account=Account(self.credentials, self.transport([{'access_token':'fresh','refresh_token':'rotated','expires_in':300}],calls))
        account.persist(dict(access_token='expired', refresh_token='old', expires_at=0, cache_id='snapshot',remember=True))
        self.assertEqual(account.token(), 'fresh')
        self.assertEqual(Account(self.credentials).load()['refresh_token'], 'rotated')
        self.credentials.save('different-client','other-secret',remember=False)
        self.assertIsNone(account.load())

    def test_pkce_state_and_scope_and_no_token_logs(self):
        urls=[]; logs=[]; calls=[]; status_codes=[]
        account=Account(self.credentials, self.transport([{'access_token':'private-token','refresh_token':'private-refresh','expires_in':300,'scope':'collection.read'}], calls))
        class FakeServer:
            def __init__(self, address, handler): self.handler=handler; self.count=0
            def __enter__(self): return self
            def __exit__(self,*args): pass
            def handle_request(self):
                self.count+=1
                params=parse_qs(urlparse(urls[0]).query)
                args={'state': params['state'][0] if self.count>1 else 'wrong', 'code':'one-time-code'}
                class Request:
                    path='/callback?' + urlencode(args)
                    wfile=io.BytesIO()
                    def send_response(self, code): status_codes.append(code)
                    def send_header(self,*args): pass
                    def end_headers(self): pass
                self.handler.do_GET(Request())
        with patch('library_manager.account.HTTPServer',FakeServer):
            account.connect(urls.append,lambda:False,logs.append,remember=False)
        params=parse_qs(urlparse(urls[0]).query)
        token_request=parse_qs(calls[0].data.decode())
        challenge=base64.urlsafe_b64encode(hashlib.sha256(token_request['code_verifier'][0].encode()).digest()).rstrip(b'=').decode()
        self.assertEqual(params['code_challenge'],[challenge]); self.assertEqual(set(params['scope'][0].split()), {'collection.read', 'search.read'})
        self.assertEqual(params['redirect_uri'],[REDIRECT]); self.assertEqual(status_codes,[400,200])
        self.assertNotIn('private-token',' '.join(logs)); self.assertNotIn('one-time-code',' '.join(logs))
        self.assertEqual(self.backend.values,{})
        self.assertEqual(account.token(), 'private-token')
