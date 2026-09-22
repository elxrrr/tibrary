import copy,json,tempfile,unittest
from pathlib import Path
from unittest.mock import Mock,patch
from PySide6.QtCore import Qt,QUrl
from PySide6.QtWidgets import QApplication
from library_manager.ui import Window,reveal_in_file_manager
from library_manager.virtual_table import VirtualTable
from library_manager.core import Store,scan
from library_manager.maintenance import inspect_file
from library_manager.tag_review import check_album_tags
from library_manager.linking import save_result,compatible_tags
from library_manager.link_statistics import link_statistics
from test_release_update import audio_file,remote
app=QApplication.instance() or QApplication([])

class SelectionIdentityTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name).resolve()/'music';self.store=Store(Path(self.tmp.name)/'db')
    def window(self):
        w=Window(self.store,demo_mode=True);w._bg_linking_timer.stop();self.addCleanup(w.close);return w
    def test_parent_approval_cascades_and_partial_reapproval_selects_all(self):
        w=self.window();release=remote();self.store.enqueue(release,[release['tracks'][0]])
        w.expanded_queue_releases={'100'}
        w.set_queue_approval('100',False)
        payload=json.loads(self.store.rows('SELECT payload FROM queue')[0]['payload'])
        self.assertEqual(payload['selected_tracks'],[])
        self.assertTrue(all(not r['checked'] for r in w.displayed_queue_rows if r['type']=='track'))
        w.set_queue_approval('100',True)
        self.assertTrue(all(r['checked'] for r in w.displayed_queue_rows if r['type']=='track'))
        w.toggle_queue_track_selection('100','11',False);w.set_queue_approval('100',True)
        self.assertIsNone(json.loads(self.store.rows('SELECT payload FROM queue')[0]['payload'])['selected_tracks'])
    def test_sorted_virtual_rebuild_retains_hierarchy_keys_and_order(self):
        table=VirtualTable(['Title','Coverage']);model=table.model()
        model.replace_custom([['A','2'],['B','1']],['a','b'])
        model.sort(1,Qt.SortOrder.AscendingOrder)
        model.replace_custom([['A','2'],['child',''],['B','1']],['a','a:t','b'],{1:'track'})
        self.assertEqual(model.keys,['b','a','a:t'])
        model.sort(1,Qt.SortOrder.DescendingOrder);self.assertEqual(model.keys,['a','a:t','b'])
    def test_missing_batch_partial_selection_only_queues_selected_tracks(self):
        w=self.window();r=remote();item=dict(release=r,state='Missing release',missing=r['tracks'])
        w._base_coverage_rows=[item];w.expanded_releases={'100'};w._render_coverage_table()
        w._missing_checked('100',True);w._missing_checked('100:11',False)
        w._base_coverage_rows=[]  # A filter must not discard already checked releases.
        with patch.object(w,'refresh_coverage'):w.queue_checked_missing()
        record=self.store.rows('SELECT * FROM queue')[0];payload=json.loads(record['payload'])
        self.assertEqual([t['id'] for t in payload['selected_tracks']],['12']);self.assertEqual(record['approved'],0)
    def test_finder_decodes_uri_and_preserves_literal_percent_filename(self):
        p=Path(self.tmp.name)/'100%20 é.flac';p.touch()
        with patch('library_manager.ui.subprocess.Popen') as launch:
            self.assertTrue(reveal_in_file_manager(QUrl.fromLocalFile(str(p)).toString()))
            self.assertEqual(launch.call_args.args[0][-1],str(p.resolve()))
            reveal_in_file_manager(str(p));self.assertEqual(launch.call_args.args[0][-1],str(p.resolve()))
    def test_more_than_eight_editions_all_retained_without_credit_poisoning(self):
        rows=[inspect_file(audio_file(self.root/'Artist/Single'/f'{n}.flac',n,2,f'Song {n}',f'TEST{n}'),self.root) for n in (1,2)]
        releases=[]
        for i in range(12):
            rel=remote(2,str(100+i));rel['album_artists']=['SATICA' if i%2 else 'Manila Killa']
            for t in rel['tracks']:t['id']=str(i*1000+int(t['id']))
            releases.append(rel)
        self.store.save_catalogue('a','GB',dict(releases=releases))
        api=Mock();api.recording_releases.return_value=[]
        results=check_album_tags(rows,self.store,'GB',api)
        for row in results:
            self.assertTrue(save_result(self.store,'GB',row))
            self.assertEqual(len(row['catalogue_choice']['equivalent_placements']),12)
            self.assertNotIn('albumartist',row['catalogue_choice']['changes'])
    def test_numeric_padding_keeps_existing_link_compatible(self):
        tags=dict(tracknumber=['1/10'],discnumber=['1'],tracktotal=['10'])
        self.assertTrue(compatible_tags(dict(tags=dict(tracknumber=['01/10'],discnumber=['01'],tracktotal=['10'])),dict(local_tags=tags)))
    def test_shared_metrics_count_whole_release_and_explicit_unlink(self):
        paths=[audio_file(self.root/'Artist/Single'/f'{n}.flac',n,2,f'Song {n}',f'TEST{n}',album_id='100') for n in (1,2)]
        scan(self.store,self.root);stats=link_statistics(self.store,'GB')
        self.assertEqual((stats['linked_releases'],stats['linked_tracks']),(1,2))
        row=inspect_file(paths[0],self.root)
        with self.store.connect() as db:db.execute('INSERT INTO track_links(path,market,stamp,payload) VALUES(?,?,?,?)',(str(paths[0]),'GB',json.dumps(row['stamp']),json.dumps(dict(ids={},status='unlinked'))))
        stats=link_statistics(self.store,'GB');self.assertEqual((stats['linked_releases'],stats['partial_releases'],stats['linked_tracks']),(0,1,1))
    def test_recording_discovery_does_not_truncate_thirteen_releases(self):
        from library_manager.tidal import Tidal
        api=Mock()
        api.entities.return_value=[{'relationships':{'albums':{'data':[{'id':str(i)} for i in range(13)]}}}]
        api.get.return_value={'data':{'attributes':{'title':'Album','numberOfItems':2}}}
        releases=Tidal.recording_releases(api,isrc='TEST')
        self.assertEqual(len(releases),13);self.assertEqual(api.get.call_count,13)
    def test_audit_native_selection_preserves_multiple_context_rows(self):
        w=self.window();page=w.optimizations_page
        rows=[dict(folder=str(self.root/str(n)),target_folder=str(self.root/'Album'),release=remote(),gained=1,duplicates=2,recoverable_bytes=1024,kind='local') for n in (1,2)]
        page.show_results(rows);page.select_all.click()
        self.assertEqual(len(page.selected_results()),2)
        from PySide6.QtCore import QItemSelectionModel
        page.table.selectionModel().select(page.table.model().index(0,0),QItemSelectionModel.SelectionFlag.Deselect|QItemSelectionModel.SelectionFlag.Rows)
        self.assertEqual(len(page.selected_results()),1)
        page.clear_checked();self.assertEqual(page.selected_results(),[])
    def test_manual_batch_never_copies_first_tracks_id_to_another_recording(self):
        w=self.window();w.nav.setCurrentRow(2)
        option=dict(id='100',track_id='11',artist='Artist',album='Album',evidence='Verified')
        rows=[dict(path=str(self.root/'Album/1.flac'),catalogue_options=[option]),
              dict(path=str(self.root/'Album/2.flac'),catalogue_options=[dict(option,id='200',track_id='22')])]
        dialog=Mock();dialog.exec.return_value=1;dialog.chosen_option=option
        with patch.object(w,'link_selected',return_value=rows),patch.object(w,'render_link_releases'),patch.object(w,'invalidate_tools_plan'),patch('library_manager.ui.OnlineAlbumLinkDialog',return_value=dialog),patch('library_manager.linking.save_result',return_value=True) as save:
            w.tools_choose_album()
            self.assertEqual(save.call_count,1);self.assertEqual(save.call_args.args[2]['catalogue_choice']['track_id'],'11')
        self.assertNotIn('catalogue_choice',rows[1])
    def test_health_and_remediation_are_separate(self):
        w=self.window();self.assertEqual(w.stack.count(),17)
        for key in ('metadata','artwork'):self.assertTrue(w.stack.widget(13).isAncestorOf(w.tools_summary_buttons[key]))
        self.assertTrue(w.tools_page_widget.isAncestorOf(w.health_audit_cards['mqa']))
        w.resize(1400,1000);w.show();w.nav.setCurrentRow(13);app.processEvents();w.grab().save('/tmp/tibrary-remediation-final.png')
