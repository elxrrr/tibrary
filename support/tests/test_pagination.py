"""Regression coverage for TIDAL's documented API-relative next links."""
import io
import json
import unittest
from urllib.parse import parse_qs, urlparse
from library_manager.tidal import Tidal, CatalogueError


class PaginationTests(unittest.TestCase):
    def run_pages(self, link):
        calls = []
        responses = [
            {'data': [{'type':'artists','id':'1','attributes':{'name':'First'}}], 'links': {'next':link}},
            {'data': [{'type':'artists','id':'2','attributes':{'name':'Second'}}]},
        ]
        def transport(request, **kwargs):
            calls.append(request)
            return io.BytesIO(json.dumps(responses.pop(0)).encode())
        api = Tidal(user_token=lambda force: 'private-token', transport=transport)
        self.calls = calls
        result = api.favourite_artists()
        return result, calls

    def test_documented_unversioned_root_link(self):
        result, calls = self.run_pages('/userCollectionArtists/me/relationships/items?page[cursor]=second')
        self.assertEqual([a['id'] for a in result], ['1','2'])
        self.assertEqual(urlparse(calls[1].full_url).path, '/v2/userCollectionArtists/me/relationships/items')
        self.assertEqual(parse_qs(urlparse(calls[1].full_url).query)['include'], ['items'])

    def test_supported_link_forms_preserve_cursor_and_context(self):
        path = 'userCollectionArtists/me/relationships/items'
        for link in [f'/{path}?page[cursor]=a%2Bb%2Fc%3D', f'/v2/{path}?page[cursor]=a%2Bb%2Fc%3D',
                     f'{path}?page[cursor]=a%2Bb%2Fc%3D', '?page[cursor]=a%2Bb%2Fc%3D',
                     f'https://openapi.tidal.com/v2/{path}?page[cursor]=a%2Bb%2Fc%3D',
                     f'//openapi.tidal.com/v2/{path}?page[cursor]=a%2Bb%2Fc%3D',
                     {'href':f'/{path}?page[cursor]=a%2Bb%2Fc%3D'}]:
            with self.subTest(link=link):
                result, calls = self.run_pages(link)
                query = parse_qs(urlparse(calls[1].full_url).query)
                self.assertEqual(query['page[cursor]'], ['a+b/c='])
                self.assertEqual(query['include'], ['items'])
                self.assertEqual(len(result), 2)

    def test_host_and_scheme_guards_remain_closed(self):
        for link in ['https://evil.example/v2/userCollectionArtists/me', '//evil.example/v2/anything',
                     'http://openapi.tidal.com/v2/anything', 'https://openapi.tidal.com@evil.example/v2/anything',
                     '/v1/anything', '/../../anything', 'https://openapi.tidal.com.evil.example/v2/anything']:
            with self.subTest(link=link):
                with self.assertRaises(CatalogueError): self.run_pages(link)
                self.assertEqual(len(self.calls), 1, 'Must reject before forwarding the bearer token')

    def test_catalogue_page_retains_market_and_include(self):
        calls=[]
        responses=[{'data':[], 'links':{'next':'/artists/1/relationships/albums?page[cursor]=two'}}, {'data':[]}]
        def transport(request, **kwargs):
            calls.append(request.full_url); return io.BytesIO(json.dumps(responses.pop(0)).encode())
        api=Tidal(transport=transport); api.token='test'; api.expires=float('inf')
        api.entities('artists/1/relationships/albums','albums','albums')
        query=parse_qs(urlparse(calls[1]).query)
        self.assertEqual(query['countryCode'], ['GB']); self.assertEqual(query['include'], ['albums'])

    def test_malformed_link_is_not_silently_treated_as_end_of_collection(self):
        for link in ({}, {'href': None}, '', 123, '/userCollectionArtists/me/relationships/items#fragment'):
            with self.subTest(link=link), self.assertRaises(CatalogueError): self.run_pages(link)

    def test_repeated_page_is_detected_after_normalization(self):
        calls=[]
        def transport(request, **kwargs):
            calls.append(request.full_url)
            return io.BytesIO(json.dumps({'data':[], 'links':{'next':'/userCollectionArtists/me/relationships/items?include=items'}}).encode())
        api=Tidal(user_token=lambda force:'private-token', transport=transport)
        with self.assertRaisesRegex(CatalogueError, 'Repeated catalogue page'):
            api.favourite_artists()
        self.assertEqual(len(calls), 1)
