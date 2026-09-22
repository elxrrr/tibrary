import tempfile,unittest
from pathlib import Path
from unittest.mock import patch
from library_manager.core import Store
from library_manager.maintenance import inspect_file,apply_one,apply_plans,MaintenanceCancelled,audio_digest
from library_manager.linking import invalidate_related_links_batch
from test_maintenance import fixture

class CancellationTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name).resolve();self.path=self.root/'song.flac';fixture(self.path)
        self.store=Store(self.root/'db');self.row=inspect_file(self.path,self.root)
        self.row.update(target=str(self.path),changes={'date':['2024']},blocked='')
    def test_cancel_during_verification_preserves_original_and_cleans_temporary(self):
        original=self.path.read_bytes();calls=0
        def cancel():
            nonlocal calls
            calls+=1;return calls>=4
        with self.assertRaises(MaintenanceCancelled):apply_one(self.row,self.store,cancel)
        self.assertEqual(self.path.read_bytes(),original)
        self.assertEqual(list(self.root.glob('.library-tags-*')),[])
    def test_cancelled_batch_is_not_reported_as_failed(self):
        with patch('library_manager.maintenance.apply_one',side_effect=MaintenanceCancelled):
            logs=[];result=apply_plans([self.row],self.store,progress=logs.append)
        self.assertIn('0 failed',result);self.assertIn('1 not processed',result)
        self.assertIn('Updating tags: date',logs[0]);self.assertNotIn('applied',logs)
    def test_batch_link_invalidation_reads_links_once(self):
        with patch.object(self.store,'rows',wraps=self.store.rows) as read:
            invalidate_related_links_batch(self.store,'GB',[(self.row,self.row)]*100)
        self.assertEqual(read.call_count,1)

    def test_completed_file_is_reconciled_before_next_file_or_cancellation(self):
        seen=[];cancelled=False
        def applied(row):
            nonlocal cancelled
            seen.append(row['path']);cancelled=True
        before=audio_digest(self.path)
        result=apply_plans([self.row,self.row],self.store,cancel=lambda:cancelled,on_applied=applied)
        self.assertEqual(seen,[str(self.path)])
        self.assertIn('1 files updated',result);self.assertIn('1 not processed',result)
        self.assertEqual(audio_digest(self.path),before)
