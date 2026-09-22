"""Check our upstream adapter interface without an account or network calls."""
import inspect
from tidaler.config import Tidal,Settings
from tidaler.model.cfg import Settings as ModelSettings
from tidaler.download import Download
from tidalapi import Session,Quality
from tidalapi.media import Track
from tidaler.constants import CoverDimensions

for name in ('login_token','login_finalize'):
    assert callable(getattr(Tidal,name,None)),name
for name in ('pkce_login_url','pkce_get_auth_token','process_auth_token','track','album'):
    assert callable(getattr(Session,name,None)),name
for name in ('high_lossless','hi_res_lossless'):
    assert hasattr(Quality,name),name
for name in ('metadata_replay_gain','lyrics_embed','use_primary_album_artist','symlink_to_track','metadata_cover_embed','metadata_cover_dimension'):
    assert name in ModelSettings.__dataclass_fields__,name
assert int(CoverDimensions.Px1280)==1280
for name in ('bpm','key','key_scale'):assert hasattr(Track,name),name
assert 'file_template' in inspect.signature(Download.item).parameters
from tidaler.metadata import Metadata
assert {'path_file','target_upc','albumartist','artists','bpm','initial_key','totaltrack','totaldisc'} <= set(inspect.signature(Metadata).parameters), 'Download metadata adapter changed'
print('Backend adapter interfaces verified; no sign-in or downloads performed.')
