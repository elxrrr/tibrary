import unittest
from unittest.mock import patch
from PySide6.QtWidgets import QDialog,QGroupBox,QPushButton
from test_navigation_and_consolidation import NavigationConsolidationTests,app
from library_manager.core import scan
from library_manager.optimizations import validate_consolidation,dj_tag_conflicts
from mutagen.flac import FLAC

class ReviewTests(NavigationConsolidationTests):
    def test_conflicts_require_exact_review_and_do_not_change_tags(self):
        path=self.root/'Artist/Single/1.flac';audio=FLAC(path);audio['bpm']='123';audio.save();scan(self.store,self.root)
        plan=self.plans()[0]
        with self.assertRaisesRegex(ValueError,'BPM/key'):validate_consolidation(self.store,plan,decode=False)
        files=validate_consolidation(self.store,plan,decode=False,review_dj=True)
        page=self.w.optimizations_page
        with patch.object(QDialog,'exec',return_value=QDialog.DialogCode.Rejected):
            self.assertFalse(page.review_dj_differences(plan,files));self.assertNotIn('reviewed_dj_conflicts',plan)
        with patch.object(QDialog,'exec',return_value=QDialog.DialogCode.Accepted):
            self.assertTrue(page.review_dj_differences(plan,files))
        self.assertEqual(plan['reviewed_dj_conflicts'],dj_tag_conflicts(plan,files))
        validate_consolidation(self.store,plan,decode=False)
        self.assertEqual(FLAC(path)['bpm'],['123'])
        plan['reviewed_dj_conflicts'][0]['removed']='wrong'
        with self.assertRaisesRegex(ValueError,'BPM/key'):validate_consolidation(self.store,plan,decode=False)
    def test_recheck_controls_and_matching_panel_edges(self):
        w=self.w;buttons=[b for b in w.findChildren(QPushButton) if b.text().startswith('Recheck')]
        self.assertGreaterEqual(len(buttons),3)
        before=[b.isEnabled() for b in buttons];w.set_job_actions_busy(True)
        self.assertTrue(all(not b.isEnabled() for b in buttons));w.set_job_actions_busy(False)
        self.assertEqual([b.isEnabled() for b in buttons],before)
        w.nav.setCurrentRow(6);w.settings_tabs.setCurrentIndex(2);app.processEvents()
        groups={g.title():g for g in w.findChildren(QGroupBox)}
        a=groups['Download engine settings'];b=groups['Additional metadata source settings']
        self.assertEqual(a.mapTo(w,a.rect().bottomLeft()).y(),b.mapTo(w,b.rect().bottomLeft()).y())
        self.assertEqual(w.provider_reset.mapTo(w,w.provider_reset.rect().bottomLeft()).y(),w.download_engine_reset.mapTo(w,w.download_engine_reset.rect().bottomLeft()).y())

def load_tests(loader,tests,pattern):
 return loader.loadTestsFromNames([__name__+'.ReviewTests.'+n for n in ReviewTests.__dict__ if n.startswith('test_')])
