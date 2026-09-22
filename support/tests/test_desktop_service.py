"""Desktop protocol tests: temporary libraries only; no credentials or network."""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from library_manager.core import Store, scan
from library_manager.desktop_service import DesktopService
from test_maintenance import fixture


class DesktopServiceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name).resolve()
        self.root = self.base / "music"
        self.file = self.root / "track.flac"
        fixture(self.file)
        self.store = Store(self.base / "db")
        scan(self.store, self.root)
        self.service = DesktopService(self.store)
        self.service.inspect(str(self.root))

    def job(self, kind, **args):
        self.service.start(kind, dict(root=str(self.root), **args))
        self.service.worker.join(10)
        self.assertFalse(self.service.active())
        self.assertEqual(self.service.job["status"], "complete", self.service.job)
        return self.service.job["result"]

    def test_all_local_tables_and_pagination(self):
        for route in (
            "files",
            "links",
            "correct",
            "organise",
            "metadata",
            "artwork",
            "artists",
            "missing",
            "queue",
            "downloaded",
            "favourites",
            "mqa",
            "local",
            "online",
        ):
            with self.subTest(route=route):
                result = self.service.table(
                    dict(root=str(self.root), route=route, limit=1)
                )
                self.assertLessEqual(len(result["rows"]), 1)

    def test_number_preview_apply_preserves_audio_and_updates_index(self):
        from library_manager.maintenance import audio_digest

        digest = audio_digest(self.file)
        p = self.job("preview", action="numbers")
        preview = self.service.preview_data(p["preview_id"])
        ids = [r["id"] for r in preview["rows"] if r["affected"]]
        self.assertTrue(ids)
        self.job("apply", preview_id=p["preview_id"], ids=ids, confirmed=True)
        self.assertEqual(audio_digest(self.file), digest)
        self.assertEqual(
            self.service.snapshot(str(self.root))[0]["tags"]["tracknumber"], ["01"]
        )

    def test_preview_requires_confirmation_and_known_paths(self):
        p = self.job("preview", action="numbers")
        with self.assertRaises(ValueError):
            self.service.paths(str(self.root), ["/unknown.flac"])
        with self.assertRaises(ValueError):
            self.service.run_job(
                "apply",
                dict(
                    root=str(self.root),
                    preview_id=p["preview_id"],
                    ids=[str(self.file)],
                ),
            )

    def test_cached_navigation_does_not_read_files(self):
        with patch(
            "library_manager.maintenance.inspect_file",
            side_effect=AssertionError("Unexpected tag read"),
        ):
            for route in ("files", "correct", "organise", "links"):
                self.service.table(dict(route=route, root=str(self.root)))

    def test_multiple_ignore_and_restore(self):
        self.service.dispatch(
            "tracks.ignore",
            dict(root=str(self.root), ids=[str(self.file)], ignored=True),
        )
        self.assertEqual(
            self.service.table(
                dict(route="links", root=str(self.root), filter="ignored")
            )["total"],
            1,
        )
        self.service.dispatch(
            "tracks.ignore",
            dict(root=str(self.root), ids=[str(self.file)], ignored=False),
        )
        self.assertEqual(
            self.service.table(
                dict(route="links", root=str(self.root), filter="ignored")
            )["total"],
            0,
        )

    def test_busy_guard_and_cancellation(self):
        import threading

        gate = threading.Event()
        with patch.object(
            self.service,
            "run_job",
            side_effect=lambda *a: self.service.cancel_event.wait(2),
        ):
            self.service.start("scan", {})
            with self.assertRaises(ValueError):
                self.service.dispatch("settings.reset", {"group": "metadata"})
            self.service.dispatch("job.cancel")
            self.service.worker.join(3)
        self.assertFalse(self.service.active())

    def test_queue_cascade_persists_exact_subset(self):
        r = dict(
            id="123",
            title="Release",
            tracks=[dict(id="1"), dict(id="2")],
            track_count=2,
            tracks_loaded=True,
            available=True,
        )
        self.store.enqueue(r)
        for ids, expected in [(None, None), (["2"], ["2"]), ([], [])]:
            self.service.dispatch("queue.select", dict(selection={"123": ids}))
            row = self.service.table(dict(route="queue"))["rows"][0]
            self.assertEqual(row["selected"], expected)

    def test_consolidation_accepts_folder_ids_and_discloses_conflicts(self):
        plan = dict(
            folder=str(self.root),
            root=str(self.root),
            release=dict(id="22", artist="Artist"),
            target_folder=str(self.base / "full"),
            reviewed_dj_conflicts=[],
        )
        with (
            patch(
                "library_manager.optimizations.load_optimization_results",
                return_value=[plan],
            ),
            patch(
                "library_manager.optimizations.validate_consolidation", return_value=[]
            ),
            patch(
                "library_manager.optimizations.dj_tag_conflicts",
                return_value=[dict(field="bpm", removed="120", retained="121")],
            ),
        ):
            p = self.job("review_consolidation", ids=[str(self.root)], scope="local")
        row = self.service.preview_data(p["preview_id"])["rows"][0]
        self.assertEqual(row["id"], str(self.root))
        self.assertEqual(row["reviewed_dj_conflicts"][0]["removed"], "120")

    def test_mqa_queue_refuses_unproven_files(self):
        with self.assertRaisesRegex(ValueError, "confirmed MQA"):
            self.service.run_job(
                "queue_mqa", dict(root=str(self.root), ids=[str(self.file)])
            )
        self.assertEqual(self.store.rows("SELECT * FROM queue"), [])

    def test_release_refresh_updates_shared_cache_and_preserves_selection(self):
        from unittest.mock import Mock

        r = dict(
            id="123",
            title="Old",
            artist="Artist",
            available=True,
            tracks_loaded=True,
            tracks=[dict(id="1"), dict(id="2")],
        )
        self.store.enqueue(r, [r["tracks"][1]])
        api = Mock()
        api.release_details.return_value = dict(r, title="Updated")
        with patch.object(self.service, "api", return_value=api):
            self.job("release_details", id="123", force=True)
        self.assertTrue(api.release_details.call_args.kwargs["force"])
        self.assertEqual(self.service.release("123")["title"], "Updated")
        import json

        self.assertEqual(
            json.loads(self.store.rows("SELECT payload FROM queue")[0]["payload"])[
                "selected_tracks"
            ],
            [dict(id="2")],
        )

    def test_numeric_positions_use_slash_total_without_double_slash(self):
        row = self.service.detail(dict(root=str(self.root), path=str(self.file)))
        self.assertIn("Track 01/6", row["local_position"])

    def test_download_completion_updates_cached_library_without_full_scan(self):
        from library_manager.downloads import index_downloaded_files

        new = self.root / "new.flac"

        def download(*args, **kwargs):
            fixture(new)
            index_downloaded_files(self.store, [str(new)])
            return "Downloaded"

        with (
            patch("library_manager.downloads.download_approved", side_effect=download),
            patch(
                "library_manager.core.scan",
                side_effect=AssertionError("Unexpected full scan"),
            ),
        ):
            self.job("download")
        self.assertEqual(
            {r["path"] for r in self.service.snapshot(str(self.root))},
            {str(self.file), str(new)},
        )

    def test_batch_queue_validation_does_not_partially_enqueue(self):
        releases = {
            "1": dict(id="1", title="Good", date="2020-01-01", available=True),
            "2": dict(id="2", title="Unavailable", date="2020-01-01", available=False),
        }
        with patch.object(
            self.service, "release", side_effect=lambda ident: releases[ident]
        ):
            with self.assertRaises(ValueError):
                self.service.dispatch(
                    "queue.add", {"selection": {"1": None, "2": None}}
                )
        self.assertEqual(self.store.rows("SELECT * FROM queue"), [])

    def test_refresh_empty_artist_selection_never_expands_to_all(self):
        with self.assertRaisesRegex(ValueError, "linked artist"):
            self.service.run_job("discography", {"ids": []})

    def test_previews_restore_only_in_their_own_workflow(self):
        p = self.job("preview", action="numbers")
        self.service.table({"route": "files", "root": str(self.root)})
        restored = self.service.table(
            {"route": "correct", "action": "numbers", "root": str(self.root)}
        )
        self.assertEqual(restored["preview_id"], p["preview_id"])
        different = self.service.table(
            {"route": "metadata", "root": str(self.root), "preview_id": p["preview_id"]}
        )
        self.assertIsNone(different["preview_id"])

    def test_api_session_reused_until_connection_settings_change(self):
        original = self.service.api()
        self.assertIs(original, self.service.api())
        self.service.dispatch(
            "settings.save",
            {
                "section": "provider",
                "values": dict(self.service.provider, request_interval_ms=1100),
            },
        )
        self.assertIsNot(original, self.service.api())
        self.assertAlmostEqual(self.service.pacer.interval, 1.1)


if __name__ == "__main__":
    unittest.main()
