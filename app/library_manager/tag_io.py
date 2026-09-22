"""Audio metadata boundary. Native FLAC comments preserve custom keys and repeats.

All write callers operate on reviewed temporary copies or staged downloads.
The extension releases the GIL while reading/writing and closes every file handle.
"""

from collections import UserDict
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace
import os
import struct
import sys

try:
    import _lofty
except ImportError:
    # Isolated provider environments use the app's ABI-stable native module.
    import importlib.util

    native_path = os.environ.get("TIBRARY_NATIVE_MODULE")
    if not native_path:
        raise
    spec = importlib.util.spec_from_file_location("_lofty", native_path)
    _lofty = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(_lofty)
    sys.modules["_lofty"] = _lofty


class Tags(UserDict):
    """Case-insensitive, multiple-value Vorbis comment mapping."""

    def __getitem__(self, key):
        return self.data[key.lower()]

    def __setitem__(self, key, value):
        self.data[key.lower()] = [value] if isinstance(value, str) else list(value)

    def __delitem__(self, key):
        del self.data[key.lower()]

    def __contains__(self, key):
        return key.lower() in self.data


@dataclass
class Picture:
    type: int = 0
    mime: str = ""
    desc: str = ""
    width: int = 0
    height: int = 0
    depth: int = 0
    colors: int = 0
    data: bytes = b""

    @classmethod
    def from_bytes(cls, data):
        offset = 0

        def number():
            nonlocal offset
            value = struct.unpack_from(">I", data, offset)[0]
            offset += 4
            return value

        def blob():
            nonlocal offset
            length = number()
            value = data[offset : offset + length]
            if len(value) != length:
                raise ValueError("Truncated picture block")
            offset += length
            return value

        picture = cls(
            type=number(),
            mime=blob().decode("ascii"),
            desc=blob().decode("utf-8"),
            width=number(),
            height=number(),
            depth=number(),
            colors=number(),
            data=blob(),
        )
        if offset != len(data):
            raise ValueError("Unexpected trailing picture data")
        return picture

    def write(self):
        def number(n):
            return struct.pack(">I", n)

        def blob(value):
            return number(len(value)) + value

        return (
            number(self.type)
            + blob(self.mime.encode("ascii"))
            + blob(self.desc.encode("utf-8"))
            + b"".join(
                number(n) for n in (self.width, self.height, self.depth, self.colors)
            )
            + blob(self.data)
        )


class Audio(Tags):
    """Detached FLAC snapshot with a small mapping API used by the core workflows."""

    _reader = staticmethod(_lofty.read_other)
    _writer = None

    def __init__(self, filename, *, pictures=True):
        self.filename = str(filename)
        sys.audit("open", self.filename, "r", os.O_RDONLY)
        result = self._reader(Path(filename), pictures)
        super().__init__(result["tags"])
        self._original = {k: list(v) for k, v in self.items()}
        self._picture_bytes = result["pictures"]
        self.pictures = [Picture.from_bytes(p) for p in self._picture_bytes]
        self._pictures_loaded = pictures
        length, rate, bits, channels = result["info"]
        self.info = SimpleNamespace(
            length=length, sample_rate=rate, bits_per_sample=bits, channels=channels
        )

    @property
    def tags(self):
        return self

    def clear_pictures(self):
        if not self._pictures_loaded:
            raise ValueError("Artwork was not loaded; reopen before editing it")
        self.pictures.clear()

    def add_picture(self, picture):
        if not self._pictures_loaded:
            raise ValueError("Artwork was not loaded; reopen before editing it")
        self.pictures.append(picture)

    def save(self, filename=None):
        path = Path(filename or self.filename)
        if path.resolve() != Path(self.filename).resolve():
            raise ValueError("Save must target the file used to create this snapshot")
        changes = {
            key: self.get(key, [])
            for key in self._original.keys() | self.keys()
            if self.get(key) != self._original.get(key)
        }
        pictures = [p.write() for p in self.pictures]
        picture_changes = (
            pictures
            if self._pictures_loaded and pictures != self._picture_bytes
            else None
        )
        if not changes and picture_changes is None:
            return
        # Native I/O must participate in Python's test/access audit policy.
        sys.audit("open", str(path), "r+", os.O_RDWR)
        if self._writer is None:
            raise ValueError(
                "Writing this audio format is not supported by library maintenance"
            )
        self._writer(path, changes, picture_changes)
        self._original = {k: list(v) for k, v in self.items()}
        self._picture_bytes = pictures


class FLAC(Audio):
    _reader = staticmethod(_lofty.read_flac)
    _writer = staticmethod(_lofty.write_flac)


class MP4(Audio):
    _writer = staticmethod(_lofty.write_mp4)


def open_audio(path, *, pictures=True):
    extension = Path(path).suffix.lower()
    cls = (
        FLAC
        if extension == ".flac"
        else MP4
        if extension in (".m4a", ".mp4", ".alac")
        else Audio
    )
    return cls(path, pictures=pictures)


def audio_digest(path, check_cancelled=lambda: None):
    sys.audit("open", str(path), "r", os.O_RDONLY)
    return _lofty.digest_flac(Path(path), check_cancelled)
