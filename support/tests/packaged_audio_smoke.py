import tempfile, subprocess
from pathlib import Path
import imageio_ffmpeg, soundfile as sf, numpy as np
from library_manager.tag_io import FLAC
from library_manager.download_metadata import normalize, download_path
from library_manager.maintenance import audio_digest

with tempfile.TemporaryDirectory(prefix="tibrary-audio-test-") as d:
    root = Path(d)
    original = root / "input.flac"
    out = root / "output.flac"
    sf.write(original, np.zeros((4410, 2)), 44100, subtype="PCM_16")
    subprocess.run(
        [
            imageio_ffmpeg.get_ffmpeg_exe(),
            "-v",
            "error",
            "-i",
            str(original),
            "-c:a",
            "copy",
            str(out),
        ],
        check=True,
    )
    audio = FLAC(out)
    audio.update(
        albumartist=["Artist"],
        artist=["Artist, Guest"],
        album=["Release"],
        title=["Track"],
        date=["2025"],
        tracknumber=["1"],
        tracktotal=["3"],
        discnumber=["1"],
        disctotal=["1"],
        initialkey=["8A"],
        bpm=["120"],
    )
    audio.save()
    before = audio_digest(out)
    normalize(audio)
    audio.save()
    assert audio_digest(out) == before
    assert audio["initialkey"] == ["8A"] and audio["bpm"] == ["120"]
    print(
        "Bundled FFmpeg FLAC extraction, tag normalization and DJ-tag preservation passed on temporary audio"
    )
