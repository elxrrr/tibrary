"""Read-only MQA evidence audit. Never interprets a negative short scan as proof.

Protocol reference: AudioAuditor MqaDetector.cs (Apache-2.0), derived from
purpl3F0x/MQA_identifier and Dniel97/MQA-identifier-python. See THIRD_PARTY.md.
This implementation uses integer PCM, repeated syncs and matching rate fields.
"""
from pathlib import Path
from .maintenance import fingerprint

SYNC = 0xBE0498C88
PATTERN = bytes(int(b) for b in f'{SYNC:036b}')
SCHEMA = 1


def scan_samples(samples):
    import _lofty
    if samples.ndim != 2 or samples.shape[1] != 2: return None
    match = _lofty.mqa_signal(samples.astype('<i4', copy=False).tobytes())
    if match:
        rate, studio, bit, hits = match
        return dict(original_rate=rate, studio=studio,
                    evidence=f'Repeated 36-bit stereo signal · bit {bit} · {hits} frames')
    return None


def audit_file(path):
    from .tag_io import FLAC
    audio = FLAC(path)
    result = dict(path=str(path), status='No signal found', detected=False,
                  bits=audio.info.bits_per_sample, rate=audio.info.sample_rate,
                  original_rate=None, evidence='No MQA signal in the first 3 seconds')
    tags = {k.lower(): v for k,v in (audio.tags or {}).items()}
    encoder = ' '.join(tags.get('encoder', []) + tags.get('mqaencoder', []))
    original = next(iter(tags.get('originalsamplerate', [])), '')
    if 'mqa' in encoder.lower() or tags.get('mqaencoder'):
        result.update(status='MQA tags', detected=True, evidence='MQA encoder metadata; audio signal not confirmed')
    elif original:
        result.update(status='Metadata clue', evidence='Original sample rate tag alone does not establish MQA')
    if original.isdecimal(): result['original_rate'] = int(original)
    try:
        import soundfile as sf
        with sf.SoundFile(str(path)) as stream:
            if stream.channels != 2:
                result['evidence'] += ' · non-stereo file'
                return result
            samples = stream.read(frames=stream.samplerate*3, dtype='int32', always_2d=True)
        match = scan_samples(samples)
        if match: result.update(match, status='MQA signal', detected=True)
    except (ImportError, OSError, RuntimeError) as exc:
        result.update(status='MQA tags' if result['detected'] else 'Not checked',
                      evidence=result['evidence']+' · PCM inspection unavailable: '+str(exc))
    return result


def audit_library(store, root, cancel=lambda:False, progress=lambda s:None, force=False):
    cached = store.preferences('mqa-audit').get('files', {})
    rows = store.rows("SELECT path FROM local_files WHERE root=? AND present=1", (str(root),))
    results = []
    for index, row in enumerate(rows):
        if cancel(): break
        path = Path(row['path'])
        if path.suffix.lower() != '.flac' or path.is_symlink(): continue
        try:
            stamp = list(fingerprint(path))
            saved = cached.get(str(path), {})
            if not force and saved.get('stamp')==stamp and saved.get('schema')==SCHEMA:
                result = saved['result']
            else:
                result = audit_file(path)
                if list(fingerprint(path)) != stamp: raise ValueError('File changed during inspection; retry')
                cached[str(path)] = dict(stamp=stamp, result=result, schema=SCHEMA)
            results.append(dict(result,stamp=stamp))
        except (OSError, ValueError) as exc:
            results.append(dict(path=str(path), status='Not checked', detected=False, evidence=str(exc)))
        if index % 25 == 0: progress(f'MQA audit · {index+1:,}/{len(rows):,} files')
    store.save_preferences('mqa-audit', dict(files=cached))
    return results


def cached_audit(store, root):
    """Restore current indexed results without reading audio or starting an audit."""
    cached=store.preferences('mqa-audit').get('files',{})
    results=[]
    for row in store.rows('SELECT path,size,mtime FROM local_files WHERE root=? AND present=1',(str(root),)):
        if Path(row['path']).suffix.lower()!='.flac':continue
        saved=cached.get(row['path'],{})
        if saved.get('schema')==SCHEMA and saved.get('stamp',[])[2:4]==[row['size'],row['mtime']]:
            results.append(dict(saved['result']))
        else:
            results.append(dict(path=row['path'],status='Not checked',detected=False,evidence='No current saved audio inspection; choose Inspect library'))
    return results
