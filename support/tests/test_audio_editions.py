import io,json,unittest
from unittest.mock import patch,Mock
from library_manager.tidal import Tidal,CatalogueError
from test_selection_and_identity import SelectionIdentityTests,app
from test_release_update import audio_file
from library_manager.maintenance import inspect_file
from library_manager.tag_review import check_album_tags
from PySide6.QtCore import QItemSelectionModel
from PySide6.QtWidgets import QMenu

class AudioTests(unittest.TestCase):
    def test_video_is_counted_for_completeness_but_not_audio(self):
        data=[dict(type='tracks',id=str(n),attributes=dict(title=str(n),duration='PT3M')) for n in range(12)]+[dict(type='videos',id='video')]
        api=Tidal(transport=lambda *a,**k:io.BytesIO(json.dumps(dict(data=data)).encode()));api.token='test';api.expires=float('inf')
        result=api.release_details(dict(id='1',track_count=13,available=True))
        self.assertEqual(result['track_count'],12);self.assertEqual(result['video_count'],1)
        self.assertNotIn('video',[t['id'] for t in result['tracks']])
        self.assertEqual(api.release_details(result)['track_count'],12)
        with self.assertRaises(CatalogueError):api.release_details(dict(id='1',track_count=14,available=True))
    def test_recording_discovery_reuses_included_album_attributes(self):
        payload=dict(data=[dict(type='tracks',id='t',attributes={},relationships={'albums':{'data':[dict(type='albums',id='a')]}})],included=[dict(type='albums',id='a',attributes=dict(title='Album',numberOfItems=12))])
        calls=[]
        def transport(req,**kwargs):calls.append(req);return io.BytesIO(json.dumps(payload).encode())
        api=Tidal(transport=transport);api.token='test';api.expires=float('inf')
        self.assertEqual(api.recording_releases('ISRC')[0]['title'],'Album');self.assertEqual(len(calls),1)

class InteractionTests(SelectionIdentityTests):
    def test_right_click_ignore_preserves_multi_selection_without_rebuild(self):
        from library_manager.core import scan
        for n in (1,2):audio_file(self.root/f'{n}.flac',n,2,f'Song {n}',f'TEST{n}')
        scan(self.store,self.root)
        w=self.window();w.link_releases_root.addItem(str(self.root),str(self.root));w.link_releases_root.setCurrentIndex(w.link_releases_root.findData(str(self.root)))
        w.nav.setCurrentRow(2);w.show();app.processEvents()
        w.link_releases_root.setCurrentIndex(w.link_releases_root.findData(str(self.root)));w.render_link_releases();w.link_filter.setCurrentText('All files');app.processEvents()
        table=w.link_table
        self.assertEqual(table.rowCount(),2)
        for n in (0,1):table.selectionModel().select(table.model().index(n,0),QItemSelectionModel.SelectionFlag.Select|QItemSelectionModel.SelectionFlag.Rows)
        from PySide6.QtCore import QTimer
        from PySide6.QtWidgets import QApplication
        clicked=[]
        def execute():
            menu=QApplication.activePopupWidget()
            if isinstance(menu,QMenu):
                action=next(a for a in menu.actions() if a.text().startswith('Ignore selected tracks'))
                clicked.append(action.text());action.trigger();menu.close()
        QTimer.singleShot(50,execute)
        QTimer.singleShot(1000,lambda:QApplication.activePopupWidget() and QApplication.activePopupWidget().close())
        with patch.object(w,'render_link_releases') as render:
            w._link_table_context_menu(table.visualRect(table.model().index(0,5)).center());render.assert_not_called()
        self.assertEqual(clicked,['Ignore selected tracks (2)'])
        self.assertEqual(len(self.store.ignored_local_files()),2)
    def test_blocked_search_emits_result(self):
        row=inspect_file(audio_file(self.root/'a.flac',1,1,'A','TEST'),self.root);row['blocked']='File needs inspection'
        events=[];checked=[]
        check_album_tags([row],self.store,'GB',Mock(),progress=events.append,on_result=checked.append)
        self.assertEqual(len(checked),1);self.assertIn('Cannot inspect',checked[0]['catalogue_note'])

def load_tests(loader,tests,pattern):
 return unittest.TestSuite([loader.loadTestsFromTestCase(AudioTests),loader.loadTestsFromNames([__name__+'.InteractionTests.'+n for n in InteractionTests.__dict__ if n.startswith('test_')])])
