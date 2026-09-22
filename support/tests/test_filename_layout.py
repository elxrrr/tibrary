import tempfile
import unittest
import unicodedata
from pathlib import Path
from unittest.mock import patch
from library_manager.maintenance import destination,replan,validate_targets
from library_manager.organisation import safe_component,layout_path,validate_layout,path_tag_notes

class FilenameLayoutTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup);self.root=Path(self.temp.name)
        self.tags=dict(albumartist=['Jack Ü'],album=['1000 gecs'],title=['gec 2 Ü?'],tracknumber=['10'],date=['2020'])
    def test_obsolete_substitutions_are_repaired_from_tags(self):
        for name in ['10 - gec 2 Ü -.flac','10 - gec 2 Ü_.flac']:
            path=self.root/'Jack Ü'/'1000 gecs (2020)'/name
            self.assertEqual(destination(self.root,self.tags,current_path=path).name,'10 - gec 2 Ü.flac')
        self.assertEqual(safe_component('Music??'),'Music')
        self.assertEqual(safe_component('Haunt // Bed'),'Haunt - Bed')
    def test_case_and_equivalent_unicode_are_not_false_repairs(self):
        path=self.root/unicodedata.normalize('NFD','jack ü')/'1000 gecs (2020)'/unicodedata.normalize('NFD','10 - gec 2 Ü.flac')
        row=dict(path=str(path),root=str(self.root),tags=self.tags,blocked='',has_artwork=True)
        plan=replan([row],organise=True)[0]
        self.assertEqual(plan['path'],plan['target'])
        self.assertNotIn('Artist folder differs from album artist',plan['issues'])
    def test_real_artist_change_keeps_equivalent_filename(self):
        path=self.root/'Wrong'/'1000 gecs (2020)'/unicodedata.normalize('NFD','10 - gec 2 Ü.flac')
        target=destination(self.root,self.tags,current_path=path)
        self.assertEqual(target.parent.parent.name,'Jack Ü');self.assertEqual(target.name,path.name)
    def test_accents_are_not_removed_or_guessed(self):
        path=self.root/'Jack U'/'1000 gecs (2020)'/'10 - gec 2 U?.flac'
        self.assertNotEqual(destination(self.root,self.tags,current_path=path),path)
    def test_unicode_equivalent_destinations_collide(self):
        plans=[dict(path=str(self.root/str(i)),target=str(self.root/title)) for i,title in enumerate(['Ü.flac','U\u0308.flac'])]
        validate_targets(plans);self.assertTrue(all(r['collision'] for r in plans))
    def test_custom_layout_sanitizes_tags_without_allowing_traversal(self):
        tags=dict(self.tags,genre=['Electronic'],title=['Haunt // Bed'],discnumber=['2'])
        result=layout_path(self.root,tags,'2020',True,template='{genre}/{albumartist}/{disc}/{tracknumber} - {title}')
        self.assertEqual(result.relative_to(self.root).as_posix(),'Electronic/Jack Ü/Disc 02/10 - Haunt - Bed.flac')
        for bad in ['../{title}','/{title}','{title.__class__}','{title!r}','{title}.flac']:
            with self.assertRaises(ValueError):validate_layout(bad)
        with self.assertRaises(ValueError):layout_path(self.root,dict(tags,title=['/']))
    def test_reported_mac_paths_follow_exact_tags_and_can_be_created(self):
        cases=[
            ('Klur','Summit / Odysée','Summit (Extended Mix)','2020','3','1',False,'Klur/Summit - Odysée (2020)/03 - Summit (Extended Mix).flac'),
            ('Klur','High Above / Stellation','Stellation','2024','2','1',False,'Klur/High Above - Stellation (2024)/02 - Stellation.flac'),
            ('CROSSTALK','CT-1 (Deluxe)','FoV_v2.0 [CHEDI.hack::]','2023','9','1',False,'CROSSTALK/CT-1 (Deluxe) (2023)/09 - FoV_v2.0 [CHEDI.hack].flac'),
            ('In the blue shirt','in my own way e.p.','Footloose','2020','5','1',False,'In the blue shirt/in my own way e.p. (2020)/05 - Footloose.flac'),
            ('CHVRCHES','The Bones Of What You Believe (10 Year Anniversary Special Edition)','Strong Hand (Live At Ancienne Belgique / 2013)','2023','8','2',True,'CHVRCHES/The Bones Of What You Believe (10 Year Anniversary Special Edition) (2023)/Disc 02/02.08 - Strong Hand (Live At Ancienne Belgique - 2013).flac')]
        with patch('library_manager.organisation.sys.platform','darwin'):
            for artist,album,title,year,number,disc,multi,expected in cases:
                with self.subTest(title=title):
                    tags=dict(albumartist=[artist],album=[album],title=[title],tracknumber=[number],discnumber=[disc])
                    target=layout_path(self.root,tags,year,multi)
                    self.assertEqual(target.relative_to(self.root).as_posix(),expected)
                    target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(b'path validity fixture')
                    self.assertEqual(target.read_bytes(),b'path validity fixture')
                    self.assertEqual(tags['album'],[album]);self.assertEqual(tags['title'],[title])
            for legal in ('Belong - Mirth','Odysée','FoV_v2.0 [CHEDI.hack]'):
                self.assertEqual(safe_component(legal),legal)
            notes=path_tag_notes({'album':['Summit / Odysée']})
            self.assertIn('already valid',notes[0]);self.assertIn('“Summit / Odysée” → “Summit - Odysée”',notes[1])
if __name__=='__main__':unittest.main()
