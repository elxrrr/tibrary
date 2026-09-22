import unittest
from unittest.mock import patch
from types import SimpleNamespace
from library_manager.core import read_metadata, coverage, title_key
from library_manager.matching import score_candidates


class AlbumArtistTests(unittest.TestCase):
    def test_album_artist_wins_and_featured_credit_is_not_split(self):
        audio=SimpleNamespace(tags={'albumartist':['Canopy'],'artist':['Canopy, Tom Finster'],
                                   'album':['Refraction (Remixes)'],'title':['Incandescent (Tom Finster Remix)']},
                              info=SimpleNamespace(length=180))
        with patch('mutagen.File',return_value=audio):
            self.assertEqual(read_metadata('song.flac')['artist'],'Canopy')
            del audio.tags['albumartist']
            self.assertEqual(read_metadata('song.flac')['artist'],'Canopy, Tom Finster')

    def test_symbol_release_evidence_and_coverage_do_not_collide(self):
        tracks=[dict(artist='.EXA',album='$$$',title='$$$')]
        def artist(title):
            return dict(id='1',name='.EXA',releases=[dict(id='2',title=title,tracks=[],tracks_loaded=False,available=True)])
        self.assertEqual(score_candidates('.EXA',tracks,[artist('$$$')])[0]['matched_releases'],1)
        self.assertEqual(coverage(tracks,artist('$$$'))[0]['state'],'Present locally')
        for other in ('!!!',''):
            self.assertEqual(score_candidates('.EXA',tracks,[artist(other)])[0]['matched_releases'],0)
            self.assertEqual(coverage(tracks,artist(other))[0]['state'],'Missing release')
        self.assertEqual(title_key('Refraction (Remixes)'),title_key('Refraction - Remixes'))
