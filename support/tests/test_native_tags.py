"""Native metadata contract tested against independent readers and decoded audio."""

import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
import soundfile as sf
from PIL import Image
from mutagen.flac import FLAC as OracleFLAC, Picture as OraclePicture
from library_manager.tag_io import FLAC, open_audio
from library_manager.maintenance import audio_digest, MaintenanceCancelled
from library_manager.core import read_metadata
from library_manager.download_tags import DownloadMetadata


class NativeTagsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.path = self.root / "é — DJ.flac"
        samples = np.random.default_rng(32).integers(
            -32768, 32767, (88200, 2), dtype=np.int16
        )
        sf.write(self.path, samples, 44100, subtype="PCM_16")
        self.pcm = samples
        self.tags = {
            "artist": ["Performer", "Guest"],
            "albumartist": ["Album artist"],
            "title": ["Song"],
            "album": ["Album"],
            "date": ["2020-02-29"],
            "bpm": ["125.25"],
            "initialkey": ["12A"],
            "tracknumber": ["01"],
            "tracktotal": ["12"],
            "discnumber": ["01"],
            "disctotal": ["02"],
            "label": ["Label"],
            "upc": ["0012345678"],
            "isrc": ["GBTEST000001"],
            "serato_beatgrid": ["opaque bytes as text"],
            "x-custom": ["one", "", "one", "two"],
            "tidal_track_id": ["123"],
            "tidal_album_id": ["456"],
            "lyrics": ["Existing lyrics"],
        }
        audio = OracleFLAC(self.path)
        audio.update(self.tags)
        self.cover = io.BytesIO()
        Image.new("RGB", (1280, 1280), "red").save(self.cover, "JPEG")
        for kind in (3, 4):
            picture = OraclePicture()
            picture.type = kind
            picture.mime = "image/jpeg"
            picture.desc = "é cover"
            picture.width = picture.height = 1280
            picture.depth = 24
            picture.data = self.cover.getvalue()
            audio.add_picture(picture)
        audio.save()

    def test_native_reader_exact_values_repeated_tags_and_artwork(self):
        audio = FLAC(self.path)
        oracle = OracleFLAC(self.path)
        self.assertEqual(dict(audio.tags), dict(oracle.tags))
        self.assertEqual(
            [p.write() for p in audio.pictures], [p.write() for p in oracle.pictures]
        )
        self.assertEqual(audio.info.sample_rate, 44100)
        self.assertEqual(audio.info.bits_per_sample, 16)
        self.assertAlmostEqual(audio.info.length, 2)
        meta = read_metadata(self.path)
        self.assertEqual(meta["artist"], "Album artist")
        self.assertEqual(meta["track_artist"], "Performer, Guest")
        self.assertEqual(meta["bpm"], "125.25")
        self.assertEqual(meta["musical_key"], "12A")

    def test_roundtrip_keeps_unknown_blocks_audio_pictures_and_vendor(self):
        # Insert a legal application block the tag library must not discard.
        data = self.path.read_bytes()
        block = b"TEST" + b"opaque application data"
        self.path.write_bytes(
            data[:42] + b"\x02" + len(block).to_bytes(3, "big") + block + data[42:]
        )
        old = OracleFLAC(self.path)
        vendor = old.tags.vendor
        covers = [p.write() for p in old.pictures]
        digest = audio_digest(self.path)
        audio = FLAC(self.path)
        audio["date"] = ["2024"]
        del audio["lyrics"]
        audio["X-New"] = ["你好", "café"]
        audio.save()
        checked = OracleFLAC(self.path)
        expected = dict(self.tags, date=["2024"], **{"x-new": ["你好", "café"]})
        expected.pop("lyrics")
        self.assertEqual(dict(checked.tags), expected)
        self.assertEqual(checked.tags.vendor, vendor)
        self.assertEqual([p.write() for p in checked.pictures], covers)
        self.assertIn(block, self.path.read_bytes())
        self.assertEqual(audio_digest(self.path), digest)
        self.assertTrue(np.array_equal(sf.read(self.path, dtype="int16")[0], self.pcm))

    def test_no_artwork_read_cannot_accidentally_remove_covers(self):
        before = [p.write() for p in OracleFLAC(self.path).pictures]
        audio = FLAC(self.path, pictures=False)
        audio["title"] = ["New"]
        audio.save()
        self.assertEqual([p.write() for p in OracleFLAC(self.path).pictures], before)
        with self.assertRaises(ValueError):
            audio.clear_pictures()

    def test_invalid_edits_and_truncation_fail_without_writes(self):
        original = self.path.read_bytes()
        audio = FLAC(self.path)
        audio["invalid=key"] = ["x"]
        with self.assertRaises(ValueError):
            audio.save()
        self.assertEqual(self.path.read_bytes(), original)
        for payload in (b"", b"fLaC", b"fLaC\x80\x00\x00\xffshort"):
            broken = self.root / "broken.flac"
            broken.write_bytes(payload)
            with self.assertRaises((ValueError, OSError)):
                FLAC(broken)
            self.assertEqual(broken.read_bytes(), payload)

    def test_padding_growth_shrink_and_boundary_keep_audio(self):
        before = audio_digest(self.path)
        # Grow past existing padding, shrink again, then make an equal-length edit.
        for value in ("x" * 1500000, "short", "other", ""):
            audio = FLAC(self.path)
            audio["large_comment"] = [value]
            audio.save()
            self.assertEqual(OracleFLAC(self.path)["large_comment"], [value])
            self.assertEqual(audio_digest(self.path), before)
            self.assertTrue(
                np.array_equal(sf.read(self.path, dtype="int16")[0], self.pcm)
            )

    def test_native_io_participates_in_external_volume_write_guard(self):
        # Run in a subprocess: audit hooks cannot be removed from this test runner.
        code = """import sys
from library_manager.tag_io import FLAC
p=sys.argv[1]
a=FLAC(p);a['title']=['blocked']
def audit(event,args):
 if event=='open' and args[0]==p and args[2]&3:raise PermissionError('blocked')
sys.addaudithook(audit)
try:a.save()
except PermissionError:pass
else:raise AssertionError('native I/O bypassed audit')
"""
        import sys

        original = self.path.read_bytes()
        subprocess.run([sys.executable, "-c", code, str(self.path)], check=True)
        self.assertEqual(self.path.read_bytes(), original)

    def test_hash_cancellation_propagates(self):
        with self.assertRaises(MaintenanceCancelled):
            audio_digest(self.path, lambda: True)

    def test_download_adapter_uses_native_tags_and_preserves_dj_metadata(self):
        with patch(
            "mutagen.File", side_effect=AssertionError("Legacy tag reader used")
        ):
            DownloadMetadata(
                self.path,
                title="Downloaded",
                album="Album",
                albumartist=["Primary"],
                artists=["Primary", "Guest"],
                tracknumber=1,
                totaltrack=12,
                discnumber=1,
                totaldisc=1,
                date="2020-02-29",
                bpm=126,
                initial_key="Am",
                upc="00123",
                cover_data=self.cover.getvalue(),
                isrc="GBTEST000001",
            ).save()
        audio = OracleFLAC(self.path)
        self.assertEqual(audio["initialkey"], ["8A"])
        self.assertEqual(audio["bpm"], ["126"])
        self.assertEqual(audio["tracknumber"], ["01"])
        self.assertEqual(audio["disctotal"], ["01"])
        self.assertNotIn("lyrics", audio)
        self.assertEqual(audio["x-custom"], ["one", "", "one", "two"])
        self.assertEqual(audio.pictures[0].width, 1280)
        self.assertTrue(np.array_equal(sf.read(self.path, dtype="int16")[0], self.pcm))

    def test_other_scan_formats_and_mp4_roundtrip(self):
        import imageio_ffmpeg
        import mutagen
        from mutagen.mp4 import MP4, MP4FreeForm, MP4Cover

        ffmpeg = imageio_ffmpeg.get_ffmpeg_exe()
        for ext, codec in [
            ("m4a", "aac"),
            ("mp3", "libmp3lame"),
            ("wav", "pcm_s16le"),
            ("aiff", "pcm_s16be"),
        ]:
            with self.subTest(format=ext):
                path = self.root / ("song." + ext)
                subprocess.run(
                    [
                        ffmpeg,
                        "-v",
                        "error",
                        "-i",
                        str(self.path),
                        "-map_metadata",
                        "-1",
                        "-vn",
                        "-c:a",
                        codec,
                        str(path),
                    ],
                    check=True,
                )
                oracle = mutagen.File(path, easy=True)
                if ext in ("wav", "aiff"):
                    from mutagen.id3 import TPE2, TPE1, TALB, TIT2, TBPM, TKEY, TRCK

                    if oracle.tags is None:
                        oracle.add_tags()
                    for item in [
                        TPE2(text=["Primary"]),
                        TPE1(text=["Guest"]),
                        TALB(text=["Album"]),
                        TIT2(text=["Track"]),
                        TBPM(text=["126"]),
                        TKEY(text=["8A"]),
                        TRCK(text=["1/12"]),
                    ]:
                        oracle.tags.add(item)
                else:
                    oracle.update(
                        albumartist=["Primary"],
                        artist=["Guest"],
                        album=["Album"],
                        title=["Track"],
                        tracknumber=["1/12"],
                        discnumber=["1/1"],
                    )
                oracle.save()
                meta = read_metadata(path)
                self.assertEqual(meta["artist"], "Primary")
                self.assertEqual(meta["track_artist"], "Guest")
                self.assertEqual(meta["album"], "Album")
                self.assertEqual(meta["title"], "Track")
                if ext in ("wav", "aiff"):
                    self.assertEqual(meta["bpm"], "126")
                    self.assertEqual(meta["musical_key"], "8A")
                if ext != "m4a":
                    continue
                raw = MP4(path)
                raw["----:com.apple.iTunes:serato_test"] = [MP4FreeForm(b"opaque")]
                raw["----:com.apple.iTunes:initialkey"] = [MP4FreeForm(b"8A")]
                raw["covr"] = [MP4Cover(self.cover.getvalue())]
                raw.save()
                before = subprocess.check_output(
                    [ffmpeg, "-v", "error", "-i", str(path), "-f", "s16le", "-"]
                )
                native = open_audio(path)
                native["title"] = ["Changed"]
                native["tracknumber"] = ["02"]
                native["tracktotal"] = ["13"]
                native["tidal_track_id"] = ["123"]
                native["bpm"] = ["125.25"]
                native.save()
                checked = MP4(path)
                self.assertEqual(checked["trkn"], [(2, 13)])
                self.assertEqual(
                    bytes(checked["----:com.apple.iTunes:serato_test"][0]), b"opaque"
                )
                self.assertEqual(
                    bytes(checked["----:com.apple.iTunes:initialkey"][0]), b"8A"
                )
                self.assertEqual(bytes(checked["covr"][0]), self.cover.getvalue())
                self.assertEqual(read_metadata(path)["tidal_track_id"], "123")
                self.assertEqual(read_metadata(path)["bpm"], "125.25")
                from library_manager.download_metadata import companion_cover

                companion_cover(open_audio(path), self.root)
                self.assertTrue((self.root / "cover.jpg").is_file())
                after = subprocess.check_output(
                    [ffmpeg, "-v", "error", "-i", str(path), "-f", "s16le", "-"]
                )
                self.assertEqual(before, after)
