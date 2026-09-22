import unittest
from library_manager.recommendations import recommend
from library_manager.view_data import filter_coverage

class RecommendationEvidence(unittest.TestCase):
    def test_catalogue_name_is_not_primary_credit_and_rights_need_credit(self):
        local=[{'label':'Label','copyright':'2024 Label'}]
        release={'artist':'Artist','label':'Label','copyright':'2025 Label'}
        self.assertNotEqual(recommend(release,local,['Artist'],True)['badge'],'Recommended')
        release['album_artists']=['Artist']
        self.assertEqual(recommend(release,local,['Artist'],True)['badge'],'Recommended')
        release['album_artists']=['Other artist']
        self.assertNotEqual(recommend(release,local,['Artist'],True)['badge'],'Recommended')
        release['album_artists']=['Artist'];release['is_compilation']=True
        self.assertNotEqual(recommend(release,local,['Artist'],True)['badge'],'Recommended')

    def test_low_confidence_and_ignored_releases_remain_inspectable(self):
        item={'release':{'id':'1','title':'Release','artist':'Artist','date':'2020-01-01'},'artist_ids':['2'],'state':'Missing release','recommendation':{'badge':'Suspect'}}
        data={'compared':[item],'owned_dates':{'2':{'2025-01-01'}}}
        rows,_,_=filter_coverage(data,{'1':'ignored'},('All releases','','All statuses','All types'))
        self.assertEqual(rows[0]['state'],'Ignored')

    def test_cached_writers_support_album_without_label_continuity(self):
        local = [{'credits': [{'name': 'Writer One', 'role': 'Composer'},
                              {'name': 'Writer Two', 'role': 'Lyricist'}]}]
        release = {'album_artists': ['Artist'], 'tracks': [
            {'id': '1', 'credits': [{'name': 'Writer One', 'role': 'Songwriter'}]},
            {'id': '2', 'credits': [{'name': 'Writer Two', 'role': 'Composer'}]}]}
        self.assertEqual(recommend(release, local, ['Artist'], True)['badge'], 'Recommended')
        release['tracks'][1]['id'] = '1'
        self.assertEqual(recommend(release, local, ['Artist'], True)['badge'], 'Potential')
        release['album_artists'] = ['Unrelated']
        self.assertNotEqual(recommend(release, local, ['Artist'], True)['badge'], 'Recommended')
