import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from mutagen.flac import FLAC, Picture
from library_manager.core import Store
from library_manager.maintenance import plan_library, apply_one, audio_digest, set_album_artist, validate_targets, normalized_date


def fixture(path):
    path.parent.mkdir(parents=True,exist_ok=True)
    # Minimal FLAC metadata fixture with a sentinel audio payload; no decoder is used.
    stream=(4096).to_bytes(2,'big')*2+b'\0'*6+((8000<<44)|(15<<36)|8000).to_bytes(8,'big')+b'\0'*16
    path.write_bytes(b'fLaC'+b'\x80\x00\x00\x22'+stream+b'audio payload preserved byte for byte')
    audio=FLAC(path)
    audio['artist']=['Canopy, Tom Finster'];audio['albumartist']=['Wrong'];audio['album']=['Refraction (Remixes)']
    audio['title']=['Incandescent'];audio['date']=['2024-05-01T00:00:00Z'];audio['tracknumber']=['1/6']
    audio['replaygain_track_gain']=['-4.2 dB'];audio['customtag']=['one','two']
    picture=Picture();picture.mime='image/png';picture.data=b'picture bytes';audio.add_picture(picture);audio.save()


class MaintenanceTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name)/'music';self.path=self.root/'Canopy'/'Album'/'song.flac';fixture(self.path)
        self.store=Store(Path(self.temp.name)/'db')

    def test_preview_and_repair_preserve_audio_and_unrelated_tags(self):
        original=self.path.read_bytes();plan=plan_library(self.root)
        self.assertEqual(self.path.read_bytes(),original)
        set_album_artist(plan,'Canopy');digest=audio_digest(self.path)
        self.assertEqual(apply_one(plan[0],self.store),'applied')
        a=FLAC(self.path)
        self.assertEqual(a['albumartist'],['Canopy']);self.assertEqual(a['artist'],['Canopy, Tom Finster'])
        self.assertEqual(a['date'],['2024-05-01']);self.assertEqual(a['customtag'],['one','two'])
        self.assertEqual(a.pictures[0].data,b'picture bytes');self.assertEqual(audio_digest(self.path),digest)
        self.assertEqual(self.store.tracks()[0]['artist'],'Canopy')

    def test_move_and_collision_protection(self):
        plan=plan_library(self.root,organise=True);set_album_artist(plan,'Canopy');validate_targets(plan)
        target=Path(plan[0]['target']);target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(b'existing')
        with self.assertRaises(ValueError):apply_one(plan[0],self.store)
        self.assertEqual(target.read_bytes(),b'existing');self.assertTrue(self.path.exists())
        target.unlink();self.assertEqual(apply_one(plan[0],self.store),'applied')
        self.assertFalse(self.path.exists());self.assertTrue(target.exists())
        self.assertEqual(Path(self.store.tracks()[0]['path']),target)

    def test_stale_preview_and_failed_save_leave_source(self):
        row=plan_library(self.root)[0]
        self.path.write_bytes(self.path.read_bytes()+b'changed')
        with self.assertRaises(ValueError):apply_one(row,self.store)
        row=plan_library(self.root)[0];original=self.path.read_bytes()
        with patch('library_manager.tag_io.FLAC.save',side_effect=OSError('disk full')):
            with self.assertRaises(OSError):apply_one(row,self.store)
        self.assertEqual(self.path.read_bytes(),original)
        self.assertFalse(list(self.path.parent.glob('.library-tags-*')))

    def test_date_precision_and_ambiguous_values(self):
        self.assertEqual(normalized_date('2024'),'2024')
        self.assertEqual(normalized_date('2024/02/29'),'2024-02-29')
        self.assertIsNone(normalized_date('01/02/2024'))
        self.assertIsNone(normalized_date('2023-02-29'))

    def test_symlink_destination_rejected(self):
        plan=plan_library(self.root,organise=True);set_album_artist(plan,'Escaped')
        outside=Path(self.temp.name)/'outside';outside.mkdir();(self.root/'Escaped').symlink_to(outside,target_is_directory=True)
        with self.assertRaises(ValueError):apply_one(plan[0],self.store)
        self.assertFalse(list(outside.iterdir()));self.assertTrue(self.path.exists())

    def test_disc_normalisation_and_existing_unpadded_folder(self):
        from library_manager.maintenance import disc_changes, destination
        changes,issue=disc_changes({'discnumber':['02/03'],'totaldiscs':['03']})
        self.assertFalse(issue)
        self.assertEqual(changes,{'discnumber':['02'],'disctotal':['03']})
        self.assertEqual(disc_changes({'discnumber':['02'],'disctotal':['03']}),({},''))
        self.assertTrue(disc_changes({'discnumber':['2/3'],'disctotal':['4']})[1])
        tags={'albumartist':['Artist'],'album':['Album'],'title':['Song'],'tracknumber':['1'],'date':['2024'],'discnumber':['2'],'disctotal':['3']}
        self.assertEqual(destination(self.root,tags),self.root/'Artist'/'Album (2024)'/'Disc 02'/'02.01 - Song.flac')
        a=FLAC(self.path);a['discnumber']=['02/03'];a.save()
        row=plan_library(self.root)[0]
        self.assertEqual(row['changes']['discnumber'],['02'])
        apply_one(row,self.store)
        a=FLAC(self.path);self.assertEqual(a['discnumber'],['02']);self.assertEqual(a['disctotal'],['03'])
