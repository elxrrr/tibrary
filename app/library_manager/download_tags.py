"""Application-owned download tagging, independent of upstream tag implementations."""

from pathlib import Path
from .tag_io import open_audio, FLAC, Picture


class DownloadMetadata:
    """Small adapter for the downloader's Metadata(...).save() contract."""

    current_album_artist = None

    def __init__(self, path_file, target_upc=None, **values):
        self.path = Path(path_file)
        self.values = values
        self.target_upc = target_upc or {}

    def save(self):
        from .download_metadata import normalize

        audio = open_audio(self.path)
        values = dict(self.values)

        # Enforce true album artist if set in context or fallback properly
        if DownloadMetadata.current_album_artist:
            values["albumartist"] = [DownloadMetadata.current_album_artist]
        elif not values.get("albumartist") and values.get("artists"):
            values["albumartist"] = values["artists"]

        fields = dict(
            title="title",
            album="album",
            albumartist="albumartist",
            artist="artists",
            copyright="copy_right",
            tracknumber="tracknumber",
            tracktotal="totaltrack",
            discnumber="discnumber",
            disctotal="totaldisc",
            date="date",
            originaldate="date",
            composer="composer",
            isrc="isrc",
            url="url_share",
            upc="upc",
            initialkey="initial_key",
            releasetype="release_type",
        )
        for tag, field in fields.items():
            value = values.get(field) if values.get(field) is not None else values.get(tag)
            if value is not None and value != "":
                audio[tag] = (
                    [str(v) for v in value]
                    if isinstance(value, (list, tuple))
                    else [str(value)]
                )
        bpm = values.get("bpm")
        if bpm and float(bpm) > 0:
            audio["bpm"] = [str(bpm)]
        if self.path.suffix.lower() in (".m4a", ".mp4"):
            audio["explicit"] = ["1" if values.get("explicit") else "0"]
        if values.get("replay_gain_write", True):
            for tag, field in dict(
                replaygain_album_gain="album_replay_gain",
                replaygain_album_peak="album_peak_amplitude",
                replaygain_track_gain="track_replay_gain",
                replaygain_track_peak="track_peak_amplitude",
            ).items():
                value = values.get(field)
                if value is not None:
                    audio[tag] = [str(value)]
        # Respect configured UPC field names while keeping the canonical application field.
        custom = self.target_upc.get("FLAC" if isinstance(audio, FLAC) else "MP4")
        if custom and values.get("upc"):
            audio[custom] = [str(values["upc"])]
        normalize(audio)
        cover = values.get("cover_data")
        if cover:
            from io import BytesIO
            from PIL import Image

            with Image.open(BytesIO(cover)) as image:
                mime = Image.MIME.get(image.format, "image/jpeg")
                width, height = image.size
            audio.clear_pictures()
            audio.add_picture(
                Picture(
                    type=3, mime=mime, width=width, height=height, depth=24, data=cover
                )
            )
        audio.save()
        return True


def install_download_adapter():
    """Bind once within the isolated downloader process; leave upstream sources intact."""
    import tidaler.download as download

    download.Metadata = DownloadMetadata
    download.FLAC = FLAC
