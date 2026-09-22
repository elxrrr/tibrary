"""Disposable real-file fixture for desktop integration tests."""
import sys
from pathlib import Path
from test_maintenance import fixture
from library_manager.core import Store,scan
from library_manager.desktop_service import DesktopService
from library_manager.demo import catalogue
root=Path(sys.argv[1]).resolve();library=root/'music'
fixture(library/'First Light.flac');fixture(library/'Drift.flac')
s=Store(root/'db');scan(s,library);DesktopService(s).inspect(str(library))
artist=catalogue()
s.mapping('Wrong','900001','confirmed','fixture',True)
for r in artist['releases']:
    r.update(tracks_loaded=True,track_count=len(r['tracks']))
    if r['available']:s.enqueue(r)

s.save_catalogue('900001','GB',artist)
