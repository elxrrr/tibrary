"""Require CD-quality FLAC; never silently publish a lossy or hi-res fallback."""
def validate_stream(manifest,stream):
    quality=getattr(stream,'audio_quality',None)
    quality=getattr(quality,'value',quality)
    codec=getattr(manifest,'codecs','').upper()
    if codec!='FLAC' or quality!='LOSSLESS':
        raise ValueError('TIDAL did not supply non-MQA lossless FLAC; this track was not downloaded.')
    if getattr(stream,'bit_depth',None)!=16 or getattr(stream,'sample_rate',None)!=44100:
        raise ValueError('TIDAL did not supply 16-bit / 44.1 kHz audio; this track was not downloaded.')


def validate_file(path):
    from .tag_io import FLAC
    if path.suffix.lower()!='.flac':raise ValueError('Expected a FLAC file; download not published.')
    audio=FLAC(path)
    if audio.info.bits_per_sample!=16 or audio.info.sample_rate!=44100:
        raise ValueError('Downloaded audio is not 16-bit / 44.1 kHz; file not published.')
