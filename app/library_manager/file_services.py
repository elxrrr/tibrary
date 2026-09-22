"""Native filesystem services shared by scans and staged metadata writes.

Inventory is read-only. Copying only accepts the caller's empty staging file;
publication, fingerprint checks and database changes remain in the reviewed workflow.
"""
import os
import sys
from pathlib import Path
from .tag_io import _lofty


def inventory(root, extensions, cancelled=lambda: False, progress=lambda _: None):
    root = Path(root).expanduser().resolve(strict=True)
    sys.audit('os.scandir', str(root))
    entries, complete = _lofty.inventory(
        root, {extension.lower().lstrip('.') for extension in extensions}, cancelled, progress
    )
    return [(str(path), size, modified) for path, size, modified in entries], complete


def copy_flac_verified(source, destination, check_cancelled):
    # Keep Python's filesystem audit hooks effective across the native boundary.
    sys.audit('open', str(source), 'r', os.O_RDONLY)
    sys.audit('open', str(destination), 'w', os.O_WRONLY)
    return _lofty.copy_flac_verified(Path(source), Path(destination), check_cancelled)
