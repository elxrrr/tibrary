# Audio fixtures

`silence.m4a` contains 50 milliseconds of generated stereo silence, encoded as AAC at 44.1 kHz. It has no third-party music or identifying metadata. Tests copy it to a temporary directory before editing tags and compare the AAC payload before and after.

The generated FLAC fixture is embedded as `MINIMAL_FLAC` in `stream_download.rs`; decoding checks verify that tag and container changes preserve its audio.
