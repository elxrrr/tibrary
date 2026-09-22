import json
import tempfile
import unittest
from pathlib import Path
from library_manager.core import Store, scan
from library_manager.matching import BatchMatcher

class SplitLibraryTests(unittest.TestCase):
    def test_partial_favourites_continue_and_save_multiple_ids(self):
        with tempfile.TemporaryDirectory() as directory:
            store=Store(Path(directory)/'db'); calls=[]
            def artist(ident):
                return dict(id=ident,name='Artist',releases=[dict(id=ident,title='First' if ident=='1' else 'Second',tracks=[],tracks_loaded=False)])
            class Api:
                def search(self,name): calls.append('search');return [dict(id='1',name='Artist'),dict(id='2',name='Artist'),dict(id='3',name='No match')]
                def artist(self,ident,progress):
                    calls.append(ident)
                    return artist(ident) if ident!='3' else dict(id='3',name='No match',releases=[])
            tracks=[dict(album=a,title='Song') for a in ('First','Second','Third')]
            matcher=BatchMatcher(store,Api(),'GB',lambda *args:[dict(id='1',name='Artist')])
            matcher.run([('Artist',tracks)])
            self.assertIn('search',calls)
            self.assertEqual({m['tidal_id'] for m in store.linked_mappings()},{'1','2'})
            self.assertEqual(Store(store.path).linked_mappings(),store.linked_mappings())
            store.mapping('Artist',None,'unlinked','User',True)
            self.assertEqual(store.linked_mappings(),[])

    def test_force_rereads_unchanged_tags_and_moves_regroup(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'music';root.mkdir();path=root/'old.flac';path.write_bytes(b'a')
            store=Store(Path(directory)/'db');name=['Old']
            reader=lambda path:dict(artist=name[0],album='Album',title='Song')
            scan(store,root,reader);name[0]='SONIN'
            self.assertEqual(scan(store,root,reader)['unchanged'],1)
            self.assertEqual(scan(store,root,reader,force=True)['read'],1)
            self.assertEqual(list(store.artists()),['SONIN'])
            path.rename(root/'moved.flac');scan(store,root,reader)
            self.assertEqual(len(store.tracks()),1)
            self.assertEqual(list(store.artists()),['SONIN'])
