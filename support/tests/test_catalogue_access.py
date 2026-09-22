import io
import json
import time
import unittest
from urllib.error import HTTPError
from library_manager.tidal import Tidal, CatalogueError
from library_manager.account import Account
from library_manager.credentials import Credentials
from test_connection import MemoryKeychain


class CatalogueAccessTests(unittest.TestCase):
    def transport(self, responses, calls):
        def send(request, **kwargs):
            calls.append(request)
            response=responses.pop(0)
            if isinstance(response, Exception): raise response
            return io.BytesIO(json.dumps(response).encode())
        return send

    def test_unavailable_favourite_does_not_abort_later_pages(self):
        calls=[]; events=[]
        responses=[{'data':[{'type':'artists','id':'removed'}], 'links':{'next':'/userCollectionArtists/me/relationships/items?page[cursor]=next'}},
                   HTTPError('https://openapi.tidal.com/v2/artists/removed',404,'gone',{},None),
                   {'data':[{'type':'artists','id':'available','attributes':{'name':'100 gecs'}}]}]
        api=Tidal(user_token=lambda force:'user-token', transport=self.transport(responses,calls),progress=events.append)
        self.assertEqual(api.favourite_artists(), [{'id':'available','name':'100 gecs'}])
        self.assertEqual(len(calls),3)
        self.assertIn('1 unavailable favourites skipped', events[-1])

    def test_collection_page_404_still_aborts(self):
        api=Tidal(user_token=lambda force:'user-token',transport=self.transport([HTTPError('https://openapi.tidal.com/v2/userCollectionArtists/me',404,'gone',{},None)],[]))
        with self.assertRaises(CatalogueError): api.favourite_artists()

    def test_permission_failure_is_not_skipped_as_missing_artist(self):
        responses=[{'data':[{'type':'artists','id':'blocked'}]}, HTTPError('https://openapi.tidal.com/v2/artists/blocked',403,'denied',{},None)]
        api=Tidal(user_token=lambda force:'user-token',transport=self.transport(responses,[]))
        with self.assertRaises(CatalogueError) as error: api.favourite_artists()
        self.assertEqual(error.exception.status,403)

    def test_album_track_404_remains_fatal(self):
        responses=[{'data':[{'type':'tracks','id':'missing'}]}, HTTPError('https://openapi.tidal.com/v2/tracks/missing',404,'gone',{},None)]
        api=Tidal(user_token=lambda force:'user-token',transport=self.transport(responses,[]))
        with self.assertRaises(CatalogueError): api.entities('albums/1/relationships/items','tracks','items')

    def test_search_uses_account_bearer_without_client_authentication(self):
        calls=[]
        responses=[{'data':[{'id':'opaque'}]}, {'data':[{'id':'1','type':'artists','attributes':{'name':'100 gecs'}}]}]
        api=Tidal(search_user_token=lambda force:'search-account-token', transport=self.transport(responses,calls))
        self.assertEqual(api.search('100 gecs')[0]['id'],'1')
        self.assertTrue(all(r.get_header('Authorization')=='Bearer search-account-token' for r in calls))
        self.assertTrue(all('openapi.tidal.com' in r.full_url for r in calls))

    def test_collection_only_session_still_works_but_search_requires_reconsent(self):
        backend=MemoryKeychain(); credentials=Credentials(lambda:backend); credentials.save('id','secret',remember=False)
        account=Account(credentials)
        account.persist({'access_token':'collection-token','expires_at':time.time()+300,'scope':'collection.read','cache_id':'test','remember':False})
        self.assertEqual(account.token(),'collection-token')
        with self.assertRaisesRegex(CatalogueError, 'search.read'): account.search_token()
        account.profile['scope']='collection.read search.read'
        self.assertEqual(account.search_token(),'collection-token')

    def test_fresh_client_401_is_not_reported_as_expired_or_retried(self):
        calls=[];events=[]
        responses=[{'access_token':'fresh-token','expires_in':300}, HTTPError('https://openapi.tidal.com/v2/searchResults',401,'unauthorized',{},None)]
        api=Tidal(credentials=lambda:('id','secret'),transport=self.transport(responses,calls),progress=events.append)
        with self.assertRaises(CatalogueError) as error: api.search('100 gecs')
        self.assertEqual(len(calls),2)
        self.assertNotIn('expired',' '.join(events))
        self.assertIn('endpoint denied app access',str(error.exception))
