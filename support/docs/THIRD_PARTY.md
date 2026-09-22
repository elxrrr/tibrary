# MQA protocol references

The read-only audit in `app/library_manager/mqa_audit.py` implements the 36-bit stereo-XOR protocol described by:

- [AudioAuditor — MqaDetector.cs](https://github.com/Angel2mp3/AudioAuditor/blob/main/AudioAuditor.Core/Services/MqaDetector.cs), Angel2mp3, Apache License 2.0 (copy in `app/resources/AudioAuditor-LICENSE.txt`). Repeated sync detection and rate field interpretation informed this implementation. The implementation here uses integer PCM, vectorised searching, consistent rate fields, and distinct metadata-only results.
- [MQA-Toolkit](https://github.com/Angel2mp3/MQA-Toolkit), read as a reference; its application code is not bundled.
- The underlying reverse engineering is credited by AudioAuditor to Stavros Avramidis ([purpl3F0x/MQA_identifier](https://github.com/purpl3F0x/MQA_identifier)) and [Dniel97/MQA-identifier-python](https://github.com/Dniel97/MQA-identifier-python).

A three-second scan can establish repeated MQA signal evidence; its absence cannot certify an entire recording as non-MQA. Metadata tags can be inaccurate. Original rate is reported from an embedded field or explicitly labelled tag, never inferred from encoded bit depth.
