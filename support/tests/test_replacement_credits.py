import copy
import unittest
from unittest.mock import patch
from library_manager.release_matching import replacement_matches
from library_manager.maintenance import detect_superseded_singles


class ReplacementCredits(unittest.TestCase):
    def rows(self, same_folder=False):
        source=dict(path='/music/Jon/Single/01.flac',duration=248.28,
                    tags=dict(albumartist=['Jon Hopkins'],artist=['Jon Hopkins, Purity Ring'],
                              album=['Single'],title=['Breathe This Air'],isrc=['GBCEL1300414'],tracknumber=['1']))
        retained=copy.deepcopy(source)
        retained.update(path='/music/Jon/Immunity/06.flac')
        retained['tags'].update(album=['Immunity'],tracknumber=['6'],discnumber=['2'])
        other=copy.deepcopy(retained);other['path']='/music/Jon/Immunity/02.flac';other['tags'].update(title=['Other'],isrc=['OTHER'],tracknumber=['2'])
        if same_folder:source['path']='/music/Jon/Immunity/01.flac'
        return source,retained,other

    def test_featured_version_retained_in_both_cleanup_paths(self):
        for same_folder in (False,True):
            rows=self.rows(same_folder);source,retained,_=rows
            self.assertIn(source['path'],detect_superseded_singles(rows))
            retained['tags']['artist']=['Jon Hopkins']
            self.assertNotIn(source['path'],detect_superseded_singles(rows))

    def test_shared_isrc_cannot_override_conflicts(self):
        source,retained,_=self.rows()
        for field,value in [('artist',['Jon Hopkins']),('artist',[]),('title',['Breathe This Air (Instrumental)']),
                            ('title',['Breathe This Air (Remaster 2023)']),('isrc',['OTHER'])]:
            different=copy.deepcopy(retained);different['tags'][field]=value
            self.assertFalse(replacement_matches(source,different),(field,value))
        retained['duration']=329.9
        self.assertFalse(replacement_matches(source,retained))

    def test_credit_order_and_list_format_do_not_hide_identical_recording(self):
        source,retained,_=self.rows()
        retained['tags']['artist']=['Purity Ring','Jon Hopkins']
        self.assertTrue(replacement_matches(source,retained))
        indexed=dict(title='Breathe This Air',isrc='GBCEL1300414',duration=248.28,artist='Jon Hopkins')
        self.assertFalse(replacement_matches(source,indexed))
        indexed['track_artist']='Jon Hopkins, Purity Ring'
        self.assertTrue(replacement_matches(source,indexed))

    def test_preview_identifies_retained_disc_and_performers(self):
        source,retained,other=self.rows()
        reason=detect_superseded_singles([source,retained,other])[source['path']]['reason']
        self.assertIn('disc 2, track 6',reason)
        self.assertIn('Purity Ring',reason)
