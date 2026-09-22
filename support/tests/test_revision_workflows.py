import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from library_manager.core import Store
from library_manager.downloads import download_approved
from library_manager.download_bridge import publish_files
from library_manager.maintenance import plan_library,replan,safe_component,set_album_artist
from test_maintenance import fixture

class RevisionTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.base=Path(self.temp.name);self.store=Store(self.base/'db')
    def release(self,ident='1'):
        return dict(id=ident,artist='A',title='Album',available=True,tracks=[dict(id='11',title='One'),dict(id='12',title='Two')],tracks_loaded=True)
    def factory(self,code):
        return lambda command,**kwargs:subprocess.Popen([sys.executable,'-u','-c',code],**kwargs)
    def test_replanning_uses_snapshot_and_preserves_manual_changes(self):
        root=self.base/'music';fixture(root/'A'/'Album'/'song.flac')
        plan=plan_library(root);set_album_artist(plan,'Fixed')
        with patch('library_manager.maintenance.FLAC',side_effect=AssertionError('Unexpected reread')):
            new=replan(plan,dates=False,organise=True)
            self.assertNotIn('date',new[0]['changes']);self.assertEqual(new[0]['changes']['albumartist'],['Fixed'])
            self.assertIn('/Fixed/',new[0]['target'])
        self.assertEqual(safe_component('Belong / Mirth'),'Belong - Mirth')
        self.assertEqual(safe_component('Belong - Mirth'),'Belong - Mirth')
    def test_track_selection_replaces_queue_and_resets_approval(self):
        r=self.release();self.store.enqueue(r)
        with self.store.connect() as db:db.execute('UPDATE queue SET approved=1')
        self.store.enqueue(r,[r['tracks'][1]])
        row=self.store.rows('SELECT * FROM queue')[0];self.assertFalse(row['approved'])
        self.assertEqual(json.loads(row['payload'])['selected_tracks'][0]['id'],'12')
        with self.store.connect() as db:db.execute('UPDATE queue SET approved=1')
        self.assertEqual(self.store.export('txt'),'https://tidal.com/browse/track/12\n')
    def test_only_approved_items_submitted_and_explicit_success_saved(self):
        self.store.enqueue(self.release('1'),[self.release()['tracks'][1]]);self.store.enqueue(self.release('2'))
        with self.store.connect() as db:db.execute("UPDATE queue SET approved=1 WHERE id='1'")
        code="import sys,json;r=json.loads(sys.stdin.readline());assert len(r['items'])==1;assert r['items'][0]['release']['selected_tracks'][0]['id']=='12';print(json.dumps(dict(event='completed',id='1')));print(json.dumps(dict(event='finished')))"
        result=download_approved(self.store,self.base,lambda:False,lambda msg:None,lambda *a:'',process_factory=self.factory(code))
        self.assertIn('1 approved',result)
        self.assertEqual([r['decision'] for r in self.store.rows('SELECT * FROM queue ORDER BY id')],['downloaded','queued'])
    def test_failure_retains_queue(self):
        self.store.enqueue(self.release())
        with self.store.connect() as db:db.execute('UPDATE queue SET approved=1')
        with self.assertRaises(Exception):download_approved(self.store,self.base,lambda:False,lambda msg:None,lambda *a:'',process_factory=self.factory('import sys;sys.stdin.readline();sys.exit(1)'))
        self.assertEqual(self.store.rows('SELECT decision FROM queue')[0]['decision'],'queued')
    def test_publish_never_overwrites_or_follows_symlinks(self):
        stage=self.base/'stage';output=self.base/'output';stage.mkdir();output.mkdir()
        (stage/'a.flac').write_bytes(b'new');(output/'a.flac').write_bytes(b'old')
        with self.assertRaises(ValueError):publish_files(stage,output)
        self.assertEqual((output/'a.flac').read_bytes(),b'old')
        (output/'a.flac').unlink();outside=self.base/'other';outside.write_bytes(b'new');(output/'a.flac').symlink_to(outside)
        with self.assertRaises(ValueError):publish_files(stage,output)
    def test_cancel_keeps_approval_and_reaps_worker(self):
        self.store.enqueue(self.release())
        with self.store.connect() as db:db.execute('UPDATE queue SET approved=1')
        cancelled=[False]
        def progress(message):
            if message=='ready':cancelled[0]=True
        code="import sys,json,time,signal;signal.signal(signal.SIGTERM,lambda *a:sys.exit(2));sys.stdin.readline();print(json.dumps(dict(event='log',message='ready')),flush=True);time.sleep(30)"
        result=download_approved(self.store,self.base,lambda:cancelled[0],progress,lambda *a:'',process_factory=self.factory(code))
        self.assertIn('cancelled',result)
        row=self.store.rows('SELECT * FROM queue')[0]
        self.assertEqual(row['decision'],'queued');self.assertEqual(row['approved'],1)
