"""UI-independent application service. Only named operations cross the desktop boundary.

One job owns mutable library state at a time. Previews live on the trusted side;
clients approve identifiers, never supply arbitrary file/tag mutation payloads.
"""

import copy
import hashlib
import json
import os
import threading
import time
import uuid
from pathlib import Path
from .core import Store, now, is_compilation_artist
from .credentials import Credentials
from .account import Account
from .client_settings import normalized, DEFAULTS
from .tidal import RequestPacer
from .cached_tidal import CachedTidal
from .maintenance import first

LOCAL_ACTIONS = {
    "dates": ("tags", dict(dates=True, discs=False, keys=False)),
    "numbers": ("tags", dict(dates=False, discs=True, keys=False)),
    "keys": ("tags", dict(dates=False, discs=False, keys=True)),
    "lyrics": ("tags", dict(dates=False, discs=False, keys=False, remove_lyrics=True)),
    "organise": ("organise", dict(arrange_layout=True)),
    "strays": ("organise", dict(arrange_layout=False, stray=True)),
    "duplicates": ("organise", dict(arrange_layout=False, duplicate_tracks=True)),
    "singles": ("organise", dict(arrange_layout=False, superseded_singles=True)),
}


def clean(value):
    if isinstance(value, dict):
        return {str(k): clean(v) for k, v in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [clean(v) for v in value]
    if isinstance(value, Path):
        return str(value)
    return value


def neutral(message):
    import re

    return re.sub(r"(?i)python-tidal|tidaler|tidal", "Online", str(message))


class DesktopService:
    JOBS = {
        "queue_mqa",
        "queue_replacements",
        "startup",
        "scan",
        "preview",
        "apply",
        "link",
        "match_artists",
        "discography",
        "release_details",
        "metadata",
        "artwork",
        "mqa",
        "optimizations",
        "check_replacements",
        "review_consolidation",
        "consolidate",
        "connections",
        "connect_account",
        "connect_download",
        "favourites",
        "download",
        "deep_review",
        "deep_preview",
        "deep_apply",
        "component_check",
        "component_update",
        "component_rollback",
        "manual_candidate",
    }

    def __init__(self, store, emit=lambda event: None, credentials=None, demo=False):
        self.store = store
        self.emit = emit
        self.credentials = credentials or Credentials()
        self.demo = demo
        self.lock = threading.RLock()
        self.worker = None
        self.cancel_event = threading.Event()
        self.auth_event = threading.Event()
        self.job = None
        self.revision = 0
        self.logs = []
        self.snapshots = {}
        self.plans = {}
        self.latest = {}
        self._views = {}
        self._health_cache = None
        self._api_client = None
        if not demo and not store.preferences("desktop-migration").get("qt_settings"):
            import plistlib

            legacy = (
                Path.home() / "Library/Preferences/com.locallibrary.TidalManager.plist"
            )
            try:
                with legacy.open("rb") as source:
                    old_settings = plistlib.load(source)
            except (OSError, ValueError, plistlib.InvalidFileException):
                old_settings = {}
            current = store.preferences("downloads")
            if not current.get("output") and old_settings.get("download_folder"):
                store.save_preferences(
                    "downloads",
                    dict(current, output=str(old_settings["download_folder"])),
                )
            if not store.preferences("desktop"):
                theme = str(old_settings.get("appearance", "system")).lower()
                store.save_preferences(
                    "desktop",
                    dict(
                        market="GB",
                        theme=theme
                        if theme in ("light", "dark", "system")
                        else "system",
                    ),
                )
            store.save_preferences("desktop-migration", {"qt_settings": True})
        self.settings = self.store.preferences(
            "desktop", {"market": "GB", "theme": "system"}
        )
        self.market = self.settings["market"]
        self.provider = normalized(self.store.preferences("provider"))
        self.pacer = RequestPacer(self.provider["request_interval_ms"] / 1000)
        self.account = Account(self.credentials, settings=lambda: self.provider)
        self.diagnostics = self.store.preferences("desktop-diagnostics")
        self.connection_state = {
            "configured": False,
            "account": False,
            "checked": False,
        }
        self.auth_url = None
        self.auth_response = None
        self._last_progress = 0
        # Jobs never retain caller-approved filesystem mutations across process restarts.
        old = self.store.preferences("desktop-last-job")
        if old.get("status") in ("running", "cancelling"):
            self.log(
                "Previous operation was interrupted. Update the library before applying another preview."
            )

    def active(self):
        return self.worker is not None and self.worker.is_alive()

    def log(self, message):
        message = neutral(self.credentials.redact(str(message)))
        if message.startswith(
            (
                "Catalogue request",
                "Fetching ",
                "Authentication ·",
                "Authentication succeeded",
            )
        ):
            return
        current = time.monotonic()
        with self.lock:
            if self.job:
                self.job["message"] = message
            routine = message.startswith(
                (
                    "Updating tags:",
                    "Updating artwork",
                    "Organising file",
                    "Scanned ",
                    "Checking library",
                    "Checked ",
                    "Reading audio",
                    "Checking folder",
                )
            )
            if routine and current - self._last_progress < 0.5:
                return
            self._last_progress = current
            self.logs.append(dict(at=now(), message=message))
            self.logs = self.logs[-800:]
        self.emit({"event": "progress", "job": self.job, "message": message})

    def changed(self):
        with self.lock:
            self.revision += 1
            self._views = {}
        self.emit({"event": "changed", "revision": self.revision})

    def api(self):
        if self._api_client is None:
            self._api_client = CachedTidal(
                self.market,
                self.cancel_event.is_set,
                credentials=self.credentials.get,
                progress=self.log,
                search_user_token=self.account.search_token,
                pacer=self.pacer,
                settings=self.provider,
                store=self.store,
            )

        return self._api_client

    def root(self, value=None):
        roots = [
            r["root"] for r in self.store.rows("SELECT root FROM roots ORDER BY root")
        ]
        value = value or (roots[0] if roots else None)
        if value and value not in roots:
            value = str(Path(value).expanduser().resolve())
        if value not in roots:
            raise ValueError("Choose a registered library first")
        return value

    def snapshot(self, root):
        from .freshness import cached_inspection
        from .linking import attach_links

        if root not in self.snapshots:
            self.snapshots[root] = attach_links(
                cached_inspection(self.store, root), self.store, self.market
            )
            if self.demo and not self.snapshots[root]:
                for indexed in self.store.rows(
                    "SELECT path,metadata FROM local_files WHERE root=? AND present=1",
                    (root,),
                ):
                    meta = json.loads(indexed["metadata"] or "{}")
                    tags = {
                        k: [str(v)]
                        for k, v in meta.items()
                        if v is not None and k != "duration"
                    }
                    tags.setdefault("albumartist", tags.get("artist", []))
                    self.snapshots[root].append(
                        dict(
                            path=indexed["path"],
                            root=root,
                            tags=tags,
                            duration=meta.get("duration"),
                            stamp=(),
                            target=indexed["path"],
                            changes={},
                            issues=[],
                            has_artwork=False,
                            blocked="Fictional demonstration file",
                        )
                    )
            for row in self.snapshots[root]:
                row["layout"] = self.store.preferences("organisation")
        return self.snapshots[root]

    def inspect(self, root, force=False):
        from .freshness import prepare_library, save_inspection
        from .core import scan

        if force:
            scan(
                self.store,
                root,
                cancelled=self.cancel_event.is_set,
                progress=self.log,
                force=True,
            )
            self.snapshots.pop(root, None)
        rows = prepare_library(
            self.store, root, self.snapshot(root), self.cancel_event.is_set, self.log
        )
        from .linking import attach_links

        self.snapshots[root] = attach_links(rows, self.store, self.market)
        self._health_cache = None
        return self.snapshots[root]

    def paths(self, root, ids=None):
        rows = self.snapshot(root)
        if ids is None:
            return rows
        chosen = set(map(str, ids))
        known = {r["path"] for r in rows}
        if not chosen <= known:
            raise ValueError(
                "The selection changed. Refresh the table and select again."
            )
        return [r for r in rows if r["path"] in chosen]

    def require_idle(self):
        if self.active():
            raise ValueError("Another operation is running. Wait or cancel it first.")

    def refresh_health(self, force=False):
        manifest = self.store.rows(
            "SELECT path,size,mtime FROM local_files WHERE present=1 ORDER BY path"
        )
        signature = hashlib.sha256(
            json.dumps(
                [manifest, self.market, self.store.preferences("organisation")],
                sort_keys=True,
            ).encode()
        ).hexdigest()
        saved = self.store.preferences("desktop-health")
        if not force and saved.get("signature") == signature:
            self._health_cache = saved.get("counts", {})
            return
        if self.cancel_event.is_set():
            return
        self.log("Updating library summary · using cached tags")
        roots = self.store.rows("SELECT root FROM roots")
        from .library_workflows import missing_metadata, workflow_plan, has_changes
        from .mqa_audit import cached_audit

        health = {
            "correct": 0,
            "organise": 0,
            "metadata": 0,
            "artwork": 0,
            "mqa": 0,
            "local": 0,
            "online": 0,
        }
        for entry in roots:
            snapshot = self.snapshot(entry["root"])
            health["correct"] += sum(
                has_changes(r) for r in workflow_plan(snapshot, "tags")
            )
            health["organise"] += sum(
                has_changes(r) for r in workflow_plan(snapshot, "organise")
            )
            for row in snapshot:
                health["metadata"] += bool(missing_metadata(row.get("tags", {})))
                health["artwork"] += not row.get("has_artwork") or tuple(
                    row.get("cover_size") or ()
                ) != (1280, 1280)
            health["mqa"] += sum(
                bool(r.get("detected")) for r in cached_audit(self.store, entry["root"])
            )
            from .optimizations import load_optimization_results

            for name, scope in [("local", "local"), ("online", "remote")]:
                health[name] += len(
                    load_optimization_results(
                        self.store,
                        entry["root"],
                        self.market,
                        scope,
                        allow_stale=True,
                    )
                    or []
                )
        self._health_cache = health
        self.store.save_preferences(
            "desktop-health", dict(signature=signature, counts=self._health_cache)
        )

    def state(self):
        from .link_statistics import link_statistics

        cache_key = ("statistics", self.revision, self.market)
        cached_stats = self._views.get(cache_key)
        if cached_stats is None:
            cached_stats = link_statistics(self.store, self.market)
            self._views[cache_key] = cached_stats
        stats = dict(cached_stats)
        active = stats.pop("active")
        roots = []
        for r in self.store.rows("SELECT * FROM roots ORDER BY root"):
            files = self.store.rows(
                "SELECT path FROM local_files WHERE root=? AND present=1 AND metadata IS NOT NULL",
                (r["root"],),
            )
            roots.append(
                dict(
                    r, tracks=len(files), linked=sum(f["path"] in active for f in files)
                )
            )
        stats.update(
            (
                self._health_cache
                or self.store.preferences("desktop-health").get("counts", {})
            )
        )
        mappings = self.store.linked_mappings()
        artists = self.store.artists()
        mapped = {m["artist"] for m in mappings}
        return clean(
            dict(
                revision=self.revision,
                roots=roots,
                stats=dict(
                    stats,
                    artists=len(artists),
                    unresolved_artists=len(set(artists) - mapped),
                    approved_queue=len(
                        self.store.rows(
                            "SELECT id FROM queue WHERE decision='queued' AND approved=1"
                        )
                    ),
                    queued=len(
                        self.store.rows("SELECT id FROM queue WHERE decision='queued'")
                    ),
                    downloaded=len(
                        self.store.rows(
                            "SELECT id FROM queue WHERE decision='downloaded'"
                        )
                    ),
                ),
                recent_downloads=[
                    self.release_row(json.loads(q["payload"]), "Downloaded")
                    for q in self.store.rows(
                        "SELECT payload FROM queue WHERE decision='downloaded' ORDER BY updated DESC LIMIT 5"
                    )
                ],
                job=copy.deepcopy(self.job),
                logs=self.logs[-100:],
                auth_url=self.auth_url,
                settings=self.settings,
                connections=self.connection_state,
                diagnostics=self.diagnostics,
                demo=self.demo,
            )
        )

    def settings_data(self):
        from .organisation import DEFAULT_LAYOUT

        downloads = self.store.preferences(
            "downloads",
            {
                "output": str(Path.home() / "Downloads/Music"),
                "quality": "LOSSLESS",
                "cover_size": 1280,
            },
        )
        downloads["quality"] = {
            "Lossless": "LOSSLESS",
            "Hi-res lossless": "HI_RES_LOSSLESS",
            "High": "HIGH",
            "Low": "LOW",
        }.get(downloads["quality"], downloads["quality"])
        return dict(
            general=self.settings,
            provider=self.provider,
            matching=self.store.match_preferences(),
            links=self.store.preferences("release_links", {"max_age_days": 30}),
            downloads=downloads,
            organisation=self.store.preferences(
                "organisation", {"template": DEFAULT_LAYOUT}
            ),
            connections=self.connection_state,
        )

    def dispatch(self, method, args=None):
        a = args or {}
        if method == "state":
            return self.state()
        if method == "settings":
            return self.settings_data()
        if method == "logs":
            return self.logs
        if method == "table":
            return self.table(a)
        if method == "detail":
            return self.detail(a)
        if method == "preview":
            return self.preview_data(a["id"])
        if method == "job.start":
            return self.start(a["kind"], a.get("args", {}))
        if method == "job.cancel":
            self.cancel_event.set()
            if self.job and self.active():
                self.job["status"] = "cancelling"
                self.log(
                    "Cancellation requested · finishing the current safe file boundary"
                )
            return True
        if method == "auth.reply":
            self.auth_response = a.get("response", "")
            self.auth_event.set()
            return True
        if method == "shutdown":
            self.cancel_event.set()
            return {"safe": not self.active()}
        self.require_idle()
        if method == "library.add":
            root = Path(a["path"]).expanduser().resolve()
            if not root.is_dir():
                raise ValueError("Choose an existing folder")
            with self.store.connect() as db:
                db.execute("INSERT OR IGNORE INTO roots(root) VALUES(?)", (str(root),))
            self.changed()
            return {"root": str(root)}
        if method == "library.remove":
            root = self.root(a["root"])
            with self.store.connect() as db:
                db.execute("DELETE FROM local_files WHERE root=?", (root,))
                db.execute("DELETE FROM roots WHERE root=?", (root,))
            self.snapshots.pop(root, None)
            self._health_cache = None
            self.changed()
            return True
        if method == "settings.save":
            section = a["section"]
            values = a["values"]
            if section == "provider":
                self.provider = normalized(values)
                self.pacer.configure(self.provider["request_interval_ms"] / 1000)
                values = self.provider
            elif section == "desktop":
                if values.get("theme") not in ("system", "light", "dark"):
                    raise ValueError("Unknown theme")
                import re

                if not re.fullmatch("[A-Z]{2}", values.get("market", "")):
                    raise ValueError("Enter a two-letter market code")
                self.settings = values
                self.market = values["market"]
                self.snapshots = {}
            elif section == "organisation":
                from .organisation import validate_layout

                validate_layout(values.get("template", ""))
                self.plans = {}
                self.latest = {}
                self._health_cache = None
                for rows in self.snapshots.values():
                    for row in rows:
                        row["layout"] = values
            elif section == "matching":
                values = {
                    "enabled": bool(values.get("enabled", True)),
                    "threshold": max(0, min(100, int(values.get("threshold", 75)))),
                    "margin": max(0, min(100, int(values.get("margin", 25)))),
                }
            elif section == "release_links":
                values = {
                    "max_age_days": max(
                        0, min(365, int(values.get("max_age_days", 30)))
                    )
                }
            elif section == "downloads":
                if values.get("quality") not in (
                    "LOSSLESS",
                    "HI_RES_LOSSLESS",
                    "HIGH",
                    "LOW",
                ):
                    raise ValueError("Unknown audio quality")
                if int(values.get("cover_size", 1280)) not in (640, 1280):
                    raise ValueError("Unsupported cover size")
                values = dict(
                    values, output=str(Path(values["output"]).expanduser().resolve())
                )
            else:
                raise ValueError("Unknown settings section")
            self.store.save_preferences(section, values)
            if section in ("provider", "desktop"):
                self._api_client = None
            self.changed()
            return self.settings_data()
        if method == "settings.reset":
            group = a["group"]
            new = dict(self.provider)
            keys = [
                k
                for k in DEFAULTS
                if (k.startswith(("download_", "segment_")) or k == "aac_bitrate_cap")
                == (group == "downloads")
            ]
            if group not in ("downloads", "metadata"):
                raise ValueError("Unknown settings group")
            for k in keys:
                new[k] = DEFAULTS[k]
            if group == "downloads":
                downloads = self.settings_data()["downloads"]
                downloads.update(quality="LOSSLESS", cover_size=1280)
                self.store.save_preferences("downloads", downloads)
            return self.dispatch(
                "settings.save", {"section": "provider", "values": new}
            )
        if method == "credentials.save":
            if self.demo:
                raise ValueError("Credential changes are disabled in demo mode")
            self.credentials.save(a["client"], a["secret"], a.get("remember", True))
            self._api_client = None
            self.connection_state["configured"] = True
            self.changed()
            return True
        if method == "credentials.forget":
            if self.demo:
                raise ValueError("Credential changes are disabled in demo mode")
            self.credentials.forget()
            self._api_client = None
            self.connection_state["configured"] = False
            self.changed()
            return True
        if method == "account.disconnect":
            if self.demo:
                raise ValueError("Account changes are disabled in demo mode")
            self.account.disconnect()
            self._api_client = None
            self.connection_state["account"] = False
            self.changed()
            return True
        if method == "tracks.ignore":
            rows = self.paths(self.root(a.get("root")), a["ids"])
            self.store.set_local_files_ignored(
                [r["path"] for r in rows], a.get("ignored", True)
            )
            self.changed()
            return True
        if method == "tracks.unlink":
            from .linking import save_result

            rows = copy.deepcopy(self.paths(self.root(a.get("root")), a["ids"]))
            for r in rows:
                r.update(
                    catalogue_choice={},
                    catalogue_options=[],
                    catalogue_note="Manually unlinked",
                    metadata_changes={},
                    dj_checks=[],
                )
                save_result(self.store, self.market, r, manual=True)
            self.snapshots.pop(self.root(a.get("root")), None)
            self.changed()
            return True
        if method == "tracks.choose":
            from .linking import save_result

            r = copy.deepcopy(self.paths(self.root(a.get("root")), [a["path"]])[0])
            options = r.get("catalogue_options", [])
            chosen = next(
                (
                    o
                    for o in options
                    if str(o["id"]) == str(a["album_id"])
                    and str(o.get("track_id")) == str(a["track_id"])
                    and (
                        not a.get("choice_key")
                        or hashlib.sha256(
                            json.dumps(o, sort_keys=True).encode()
                        ).hexdigest()
                        == a["choice_key"]
                    )
                ),
                None,
            )
            if chosen is None:
                raise ValueError("Choose an inspected candidate")
            r["catalogue_choice"] = chosen
            if not save_result(self.store, self.market, r, manual=True):
                raise ValueError("File changed since inspection; refresh first")
            self.snapshots.pop(self.root(a.get("root")), None)
            self.changed()
            return True
        if method == "artists.choose":
            name = a["artist"]
            ids = list(dict.fromkeys(str(i) for i in a["ids"]))
            if (
                name not in self.store.artists()
                or not ids
                or not all(i.isdecimal() for i in ids)
            ):
                raise ValueError("Choose valid artist identities")
            self.store.mapping(
                name, ids[0], "confirmed", "Chosen in artist review", True, ids[1:]
            )
            self.changed()
            return True
        if method == "queue.select":
            with self.store.connect() as db:
                for ident, selection in a["selection"].items():
                    row = db.execute(
                        "SELECT payload FROM queue WHERE id=? AND decision='queued'",
                        (str(ident),),
                    ).fetchone()
                    if not row:
                        raise ValueError("Queued release is no longer available")
                    release = json.loads(row[0])
                    tracks = release.get("tracks", [])
                    if selection is None:
                        release["selected_tracks"] = None
                        approved = True
                    else:
                        ids = set(map(str, selection))
                        known = {str(t["id"]) for t in tracks}
                        if not ids <= known:
                            raise ValueError(
                                "Load the release track list before selecting tracks"
                            )
                        release["selected_tracks"] = [
                            t for t in tracks if str(t["id"]) in ids
                        ]
                        approved = bool(ids)
                    db.execute(
                        "UPDATE queue SET payload=?,approved=?,updated=? WHERE id=?",
                        (json.dumps(release), int(approved), now(), str(ident)),
                    )
            self.changed()
            return True
        if method == "queue.decision":
            if a.get("decision") == "ignored":
                for ident in a["ids"]:
                    r = self.release(ident)
                    with self.store.connect() as db:
                        db.execute(
                            "INSERT INTO queue VALUES(?,?,0,'ignored',?) ON CONFLICT(id) DO UPDATE SET approved=0,decision='ignored',updated=excluded.updated",
                            (str(ident), json.dumps(r), now()),
                        )
                self.changed()
                return True
            decision = a["decision"]
            if decision not in ("queued", "ignored", "removed"):
                raise ValueError("Unknown queue action")
            with self.store.connect() as db:
                for ident in a["ids"]:
                    if decision == "removed":
                        db.execute("DELETE FROM queue WHERE id=?", (str(ident),))
                    else:
                        db.execute(
                            "UPDATE queue SET decision=?,approved=0,updated=? WHERE id=?",
                            (decision, now(), str(ident)),
                        )
            self.changed()
            return True
        if method == "queue.add":
            pending = []
            for ident, ids in a["selection"].items():
                r = self.release(ident)
                from .view_data import release_is_out

                if not release_is_out(r):
                    raise ValueError("This release is not fully released")
                if r.get("available") is not True:
                    raise ValueError(
                        "Refresh track details to confirm release availability before queuing"
                    )
                selected = None
                if ids is not None:
                    wanted = set(map(str, ids))
                    selected = [
                        t for t in r.get("tracks", []) if str(t["id"]) in wanted
                    ]
                    if not wanted or len(selected) != len(wanted):
                        raise ValueError("Load and select valid audio tracks")
                pending.append(
                    (
                        str(ident),
                        json.dumps(
                            dict(
                                r,
                                selected_tracks=selected,
                                url=f"https://tidal.com/album/{ident}",
                            )
                        ),
                        now(),
                    )
                )
            with self.store.connect() as db:
                db.executemany(
                    "INSERT INTO queue VALUES(?,?,1,'queued',?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload,approved=1,decision='queued',updated=excluded.updated",
                    pending,
                )
            self.changed()
            return True
        if method == "queue.export":
            return {"text": self.store.export(a.get("format", "json"))}
        raise ValueError("Unknown application command")

    def start(self, kind, args):
        with self.lock:
            self.require_idle()
            if kind not in self.JOBS:
                raise ValueError("Unknown operation")
            if self.demo and kind in {
                "apply",
                "consolidate",
                "deep_apply",
                "download",
                "connect_account",
                "connect_download",
                "component_update",
                "component_rollback",
            }:
                raise ValueError(
                    "This operation is disabled in the demonstration library"
                )
            self.cancel_event.clear()
            self.job = dict(
                id=uuid.uuid4().hex,
                kind=kind,
                status="running",
                message="Starting…",
                started=time.time(),
                result=None,
            )
            self.store.save_preferences("desktop-last-job", self.job)

            def run():
                try:
                    value = self.run_job(kind, copy.deepcopy(args))
                    if (
                        kind
                        in (
                            "startup",
                            "scan",
                            "apply",
                            "consolidate",
                            "deep_apply",
                            "download",
                            "mqa",
                            "optimizations",
                            "check_replacements",
                            "link",
                            "metadata",
                        )
                        and not self.cancel_event.is_set()
                    ):
                        self.refresh_health(force=kind not in ("startup", "scan"))
                    self.job.update(
                        status="cancelled"
                        if self.cancel_event.is_set()
                        else "complete",
                        result=clean(value),
                    )
                except Exception as exc:
                    from .diagnostics import record_failure

                    record_failure(kind, exc)
                    message = neutral(self.credentials.redact(str(exc)))
                    self.job.update(
                        status="cancelled" if self.cancel_event.is_set() else "failed",
                        message=message,
                    )
                    self.log(message)
                finally:
                    self.job["finished"] = time.time()
                    self.auth_url = None
                    self.store.save_preferences("desktop-last-job", self.job)
                    self.changed()
                    self.emit({"event": "job", "job": self.job})

            self.worker = threading.Thread(
                target=run, name="tibrary-" + kind, daemon=False
            )
            self.worker.start()
            return copy.deepcopy(self.job)

    def persist_plan(self, root, operation, rows):
        ident = uuid.uuid4().hex
        self.plans[ident] = dict(
            id=ident, root=root, operation=operation, rows=rows, created=time.time()
        )
        self.latest[(root, operation)] = ident
        # Bound memory without silently reusing stale approvals.
        if len(self.plans) > 12:
            oldest = next(iter(self.plans))
            self.plans.pop(oldest)
        return {
            "preview_id": ident,
            "root": root,
            "count": len(rows),
            "operation": operation,
        }

    def preview_data(self, ident):
        if ident not in self.plans:
            raise ValueError("Preview expired; prepare it again")
        p = self.plans[ident]
        if p["operation"] == "consolidate":
            return clean(
                dict(
                    id=ident,
                    root=p["root"],
                    operation="consolidate",
                    scope=p.get("scope", "local"),
                    rows=[
                        dict(
                            id=r["folder"],
                            path=r["folder"],
                            artist=r["release"].get("artist"),
                            release=Path(r["folder"]).name,
                            target=r.get("target_folder"),
                            changes=f"{r['folder']} → Trash; keep {r.get('target_folder')}",
                            reviewed_dj_conflicts=r.get("reviewed_dj_conflicts", []),
                        )
                        for r in p["rows"]
                    ],
                )
            )
        return clean(
            dict(
                id=ident,
                operation=p["operation"],
                root=p["root"],
                rows=[
                    dict(
                        self.file_row(r),
                        reviewed_dj_conflicts=r.get("reviewed_dj_conflicts", []),
                    )
                    for r in p["rows"]
                ],
                raw=p["rows"] if p["operation"] == "deep" else [],
            )
        )

    def favourites(self, force=False):
        from .credentials import CredentialError

        try:
            profile = self.account.load()
        except CredentialError:
            return []
        if not profile:
            return []
        saved = self.store.favourites(profile["cache_id"])
        if saved and not force:
            return json.loads(saved["payload"])
        api = self.api()
        previous = api.user_token
        try:
            api.user_token = self.account.token
            artists = api.entities(
                "userCollectionArtists/me/relationships/items", "artists", "items"
            )
        finally:
            api.user_token = previous
        items = [dict(id=str(a["id"]), name=a["attributes"]["name"]) for a in artists]
        self.store.save_favourites(profile["cache_id"], items)
        return items

    def run_job(self, kind, a):
        cancel = self.cancel_event.is_set
        if kind == "startup":
            if self.demo:
                return {"message": "Demonstration library ready"}
            self.connection_state.update(
                configured=self.credentials.has_keys(),
                account=self.account.logged_in(),
                checked=True,
            )
            for row in self.store.rows("SELECT root FROM roots"):
                if cancel():
                    break
                if Path(row["root"]).is_dir():
                    self.inspect(row["root"])
            return {"message": "Library index ready"}
        if kind == "connections":
            if self.demo:
                return {"message": "Demo uses no online accounts"}
            from .connections import check_connections
            from .downloads import check_download_connection

            self.diagnostics = check_connections(
                self.credentials, self.account, self.api(), self.log, details=True
            )
            message = check_download_connection(self.store)
            self.diagnostics["metrics"]["download"] = {
                "ok": message.endswith(": connected."),
                "message": message,
            }
            self.connection_state = dict(
                configured=self.credentials.has_keys(),
                account=self.account.logged_in(),
                checked=True,
            )
            self.store.save_preferences("desktop-diagnostics", self.diagnostics)
            return self.diagnostics
        if kind == "connect_account":
            result = self.account.connect(
                lambda url: self.emit({"event": "open_url", "url": url}),
                cancel,
                self.log,
            )
            self.connection_state["account"] = self.account.logged_in()
            return result
        if kind in ("connect_download", "download"):
            from .downloads import download_approved

            def authenticate(url, cancel):
                self.auth_response = None
                self.auth_event.clear()
                self.auth_url = url
                self.emit({"event": "authentication", "url": url})
                deadline = time.monotonic() + self.provider["sign_in_timeout_sec"]
                while not cancel() and time.monotonic() < deadline:
                    if self.auth_event.wait(0.2):
                        return self.auth_response
                return None

            output = self.settings_data()["downloads"]["output"]
            try:
                return download_approved(
                    self.store,
                    output,
                    cancel,
                    self.log,
                    authenticate,
                    connect_only=kind == "connect_download",
                )
            finally:
                if kind == "download":
                    from .maintenance import inspect_file
                    from .freshness import save_inspection
                    from .linking import attach_links

                    for entry in self.store.rows("SELECT root FROM roots"):
                        root = entry["root"]
                        known = {r["path"]: r for r in self.snapshot(root)}
                        updated = []
                        for row in self.store.rows(
                            "SELECT path,size,mtime FROM local_files WHERE root=? AND present=1",
                            (root,),
                        ):
                            previous = known.get(row["path"])
                            if previous and tuple(previous.get("stamp", ()))[2:4] == (
                                row["size"],
                                row["mtime"],
                            ):
                                updated.append(previous)
                            elif Path(row["path"]).suffix.lower() == ".flac":
                                updated.append(
                                    inspect_file(
                                        row["path"],
                                        root,
                                        self.store.preferences("organisation"),
                                    )
                                )
                        self.snapshots[root] = attach_links(
                            updated, self.store, self.market
                        )
                        save_inspection(self.store, root, self.snapshots[root])
        if kind == "favourites":
            return {"artists": len(self.favourites(True))}
        if kind.startswith("component_"):
            from . import backends

            if kind == "component_check":
                return backends.check_upstream_updates()
            if kind == "component_update":
                return backends.update(cancel, self.log)
            return backends.rollback()
        if kind == "release_details":
            r = self.api().release_details(
                self.release(a["id"]), force=a.get("force", False)
            )
            self.save_release(r)
            return {"release_id": r["id"]}
        if kind == "discography":
            if "ids" in a and not a["ids"]:
                raise ValueError(
                    "Select a linked artist before refreshing its releases"
                )
            ids = set(
                a.get("ids") or [m["tidal_id"] for m in self.store.linked_mappings()]
            )
            for i, ident in enumerate(ids, 1):
                if cancel():
                    break
                self.log(f"Refreshing releases · {i}/{len(ids)}")
                r = self.api().artist(
                    str(ident), self.log, detailed=a.get("detailed", False)
                )
                self.store.save_catalogue(str(ident), self.market, r)
            return {"checked": len(ids)}
        root = self.root(a.get("root"))
        if kind == "scan":
            rows = self.inspect(root, a.get("force", False))
            return {"files": len(rows)}
        rows = self.snapshot(root)
        if not rows:
            rows = self.inspect(root)
        selected = (
            self.paths(root, a.get("ids"))
            if kind
            in (
                "link",
                "metadata",
                "artwork",
                "manual_candidate",
                "deep_review",
                "queue_mqa",
            )
            else rows
        )
        if kind == "match_artists":
            from .matching import BatchMatcher

            chosen = set(a.get("artists") or [])
            artists = self.store.artists()
            confirmed = {m["artist"] for m in self.store.linked_mappings()}
            artists = {
                name: tracks
                for name, tracks in artists.items()
                if (name in chosen if chosen else name not in confirmed)
            }
            return BatchMatcher(
                self.store,
                self.api(),
                self.market,
                lambda *_: self.favourites(),
                redact=self.credentials.redact,
            ).run(artists, cancel, self.log)
        if kind in ("link", "metadata", "artwork", "manual_candidate"):
            from .linking import link_recordings, attach_links
            from .dj_metadata import DJMetadata

            if kind == "link" and "ids" not in a:
                from .link_statistics import unresolved_paths

                scope = unresolved_paths(
                    self.store, self.market, rows, a.get("editions_only", False)
                )
                selected = [r for r in rows if r["path"] in scope]
            with DJMetadata(
                self.store, cancel, self.log, self.pacer, market=self.market
            ) as dj:
                if kind in ("link", "metadata"):
                    message = link_recordings(
                        selected,
                        self.store,
                        self.market,
                        self.api(),
                        dj.lookup,
                        cancel,
                        self.log,
                        recheck=kind == "link",
                        context_rows=rows,
                    )
                    selected = attach_links(selected, self.store, self.market)
                    if kind == "link":
                        self.snapshots[root] = attach_links(
                            rows, self.store, self.market
                        )
                        return {"message": message}
                elif kind == "manual_candidate":
                    from .tag_review import check_album_tags

                    ident = str(a["album_id"])
                    if not ident.isdecimal():
                        raise ValueError("Enter a numeric release ID")
                    release = self.api().album_tag_details({"id": ident})
                    self.save_release(release)
                    selected = check_album_tags(
                        selected,
                        self.store,
                        self.market,
                        self.api(),
                        cancel,
                        self.log,
                        context_rows=rows,
                    )
                    from .linking import save_result

                    for r in selected:
                        save_result(self.store, self.market, r)
                    self.snapshots[root] = attach_links(rows, self.store, self.market)
                    return {
                        "message": "Candidate inspected; open Choose match to review"
                    }
                else:
                    from .tag_review import check_album_tags

                    selected = check_album_tags(
                        selected,
                        self.store,
                        self.market,
                        self.api(),
                        cancel,
                        self.log,
                        covers=True,
                        context_rows=rows,
                    )
            from .library_workflows import workflow_plan

            return self.persist_plan(root, kind, workflow_plan(selected, kind))
        if kind == "preview":
            action = a["action"]
            if action not in LOCAL_ACTIONS:
                raise ValueError("Choose one local correction")
            from .library_workflows import workflow_plan

            mode, flags = LOCAL_ACTIONS[action]
            plans = workflow_plan(rows, mode, **flags)
            return self.persist_plan(root, action, plans)
        if kind == "apply":
            from .library_workflows import validate_operation, has_changes
            from .maintenance import apply_plans, inspect_file
            from .linking import (
                retain_after_apply,
                invalidate_related_links_batch,
                compatible_tags,
            )
            from .freshness import save_inspection

            p = self.plans.get(a["preview_id"])
            if not p or p["root"] != root:
                raise ValueError("Prepare a current preview first")
            if a.get("confirmed") is not True:
                raise ValueError("Review and confirm the exact file changes first")
            ids = set(a["ids"])
            plans = copy.deepcopy(
                [r for r in p["rows"] if r["path"] in ids and has_changes(r)]
            )
            if len(plans) != len(ids):
                raise ValueError("Choose only affected files in this preview")
            mode = LOCAL_ACTIONS.get(p["operation"], (p["operation"], {}))[0]
            for row in plans:
                validate_operation(row, mode)
            updates = {}
            pairs = []

            def applied(row):
                if row.get("superseded_by"):
                    updates[row["path"]] = None
                    return
                fresh = inspect_file(
                    row["target"], root, self.store.preferences("organisation")
                )
                retain_after_apply(self.store, self.market, row, fresh)
                if not compatible_tags(
                    fresh, dict(local_tags=row["tags"], duration=row.get("duration"))
                ):
                    pairs.append((row, fresh))
                updates[row["path"]] = fresh

            message = apply_plans(
                plans, self.store, cancel, self.log, on_applied=applied
            )
            invalidate_related_links_batch(self.store, self.market, pairs)
            self.snapshots[root] = [
                updates.get(r["path"], r)
                for r in rows
                if updates.get(r["path"], r) is not None
            ]
            save_inspection(self.store, root, self.snapshots[root])
            self.plans.pop(p["id"], None)
            return {"message": message}
        if kind == "queue_mqa":
            from .mqa_audit import cached_audit
            from .release_matching import recording_matches

            audits = {r["path"]: r for r in cached_audit(self.store, root)}
            releases = {}
            selections = {}
            preserve = {}
            for r in selected:
                if cancel():
                    raise ValueError("Cancelled; queue unchanged")
                if not audits.get(r["path"], {}).get("detected"):
                    raise ValueError("Select only files with confirmed MQA evidence")
                ids = r.get("linked_ids") or {}
                ident = str(ids.get("album_id") or first(r["tags"], "tidal_album_id"))
                track_id = str(
                    ids.get("track_id") or first(r["tags"], "tidal_track_id")
                )
                if not ident or not track_id:
                    raise ValueError(
                        "Link the selected recordings before queuing replacements"
                    )
                if ident not in releases:
                    releases[ident] = self.api().album_tag_details({"id": ident})
                    self.save_release(releases[ident])
                release = releases[ident]
                matches = [
                    t
                    for t in release.get("tracks", [])
                    if str(t["id"]) == track_id and recording_matches(r, t, strict=True)
                ]
                if len(matches) != 1 or release.get("available") is not True:
                    raise ValueError(
                        "Recheck the recording link and availability before replacing it"
                    )
                selections.setdefault(ident, {})[track_id] = matches[0]
                preserve.setdefault(ident, {})[track_id] = {
                    k: r["tags"][k] for k in ("bpm", "initialkey") if r["tags"].get(k)
                }
            for ident, release in releases.items():
                self.store.enqueue(
                    dict(
                        release,
                        preserve_local_dj=preserve[ident],
                        replacement_audit=dict(reason="MQA", quality="Lossless"),
                    ),
                    list(selections[ident].values()),
                )
            return {
                "message": f"{len(releases)} replacement releases queued for approval; original files retained"
            }
        if kind == "queue_replacements":
            from .optimizations import load_optimization_results
            from .release_matching import recording_matches

            plans = (
                load_optimization_results(self.store, root, self.market, "remote") or []
            )
            chosen = [p for p in plans if p["folder"] in a["ids"]]
            if len(chosen) != len(set(a["ids"])):
                raise ValueError("Refresh replacements before queuing this selection")
            for p in chosen:
                preserve = {}
                for source in p["sources"]:
                    for track in p["release"].get("tracks", []):
                        if recording_matches(source, track, strict=True):
                            preserve[str(track["id"])] = {
                                k: [source[v]]
                                for k, v in [
                                    ("bpm", "bpm"),
                                    ("initialkey", "musical_key"),
                                ]
                                if source.get(v)
                            }
                self.store.enqueue(dict(p["release"], preserve_local_dj=preserve))
            return {
                "message": f"{len(chosen)} replacements queued; original files and DJ tags retained"
            }
        if kind == "mqa":
            from .mqa_audit import audit_library

            return {
                "rows": len(
                    audit_library(
                        self.store, root, cancel, self.log, force=a.get("force", False)
                    )
                )
            }
        if kind in ("optimizations", "check_replacements"):
            from .optimizations import (
                find_optimizations,
                save_optimization_results,
                check_candidate_releases,
            )

            scope = a.get("scope", "local")
            if kind == "check_replacements":
                check_candidate_releases(
                    self.store,
                    self.market,
                    root,
                    self.api(),
                    cancel,
                    self.log,
                    limit=a.get("limit", 100),
                )
            found = find_optimizations(
                self.store, self.market, root, cancel, self.log, scope=scope
            )
            save_optimization_results(self.store, root, self.market, scope, found)
            return {"opportunities": len(found)}
        if kind in ("review_consolidation", "consolidate"):
            from .optimizations import (
                load_optimization_results,
                validate_consolidation,
                dj_tag_conflicts,
                consolidate,
            )

            plans = (
                load_optimization_results(
                    self.store, root, self.market, a.get("scope", "local")
                )
                or []
            )
            chosen = [p for p in plans if p["folder"] in a["ids"]]
            if not chosen:
                raise ValueError("Refresh optimizations and choose current results")
            if kind == "review_consolidation":
                sources = {str(Path(p["folder"]).resolve()) for p in chosen}
                if any(
                    str(Path(p.get("target_folder") or "").resolve()) in sources
                    for p in chosen
                ):
                    raise ValueError(
                        "A selected destination is also selected for removal; review these separately"
                    )
                for p in chosen:
                    inspected = validate_consolidation(
                        self.store, p, cancel=cancel, progress=self.log, review_dj=True
                    )
                    p["reviewed_dj_conflicts"] = dj_tag_conflicts(p, inspected)
                result = self.persist_plan(root, "consolidate", chosen)
                self.plans[result["preview_id"]]["scope"] = a.get("scope", "local")
                return result
            preview = self.plans.get(a["preview_id"])
            if not preview or a.get("confirmed") is not True:
                raise ValueError("Review the removal preview first")
            for p in preview["rows"]:
                if cancel():
                    break
                consolidate(self.store, self.market, p, cancel=cancel)
            self.plans.pop(preview["id"], None)
            self.inspect(root)
            from .optimizations import save_optimization_results

            save_optimization_results(
                self.store,
                root,
                self.market,
                a.get("scope", "local"),
                [p for p in plans if Path(p["folder"]).exists()],
            )
            return {"message": "Selected duplicates moved to Trash"}
        if kind == "deep_review":
            from .extended_review import discover

            tracks = [dict(r, **r.get("tags", {})) for r in selected]
            # Discovery consumes indexed metadata, not raw multi-value tags.
            indexed = {
                r["path"]: dict(json.loads(r["metadata"]), path=r["path"])
                for r in self.store.rows(
                    "SELECT path,metadata FROM local_files WHERE root=? AND present=1 AND metadata IS NOT NULL",
                    (root,),
                )
            }
            found = discover(
                [indexed[r["path"]] for r in selected if r["path"] in indexed],
                self.store,
                self.market,
                self.api(),
                cancel,
                self.log,
                query=a.get("query"),
            )
            return self.persist_plan(root, "deep", found)
        if kind == "deep_preview":
            from .extended_review import repair_preview

            p = self.plans[a["preview_id"]]
            plans = repair_preview(self.store, p["rows"][int(a["index"])], cancel)
            return self.persist_plan(root, "deep_apply", plans)
        if kind == "deep_apply":
            from .extended_review import apply_repairs

            if a.get("confirmed") is not True:
                raise ValueError("Review the repair preview first")
            p = self.plans[a["preview_id"]]
            result = apply_repairs(self.store, self.market, p["rows"], cancel, self.log)
            self.inspect(root)
            self.plans.pop(p["id"], None)
            return result
        raise ValueError("Unsupported operation")

    def release(self, ident):
        ident = str(ident)
        saved = self.store.preferences(f"tag-review:{self.market}:{ident}")
        if saved:
            return saved
        for q in self.store.rows("SELECT payload FROM queue WHERE id=?", (ident,)):
            return json.loads(q["payload"])
        for c in self.store.rows(
            "SELECT payload FROM catalogue WHERE market=?", (self.market,)
        ):
            for r in json.loads(c["payload"]).get("releases", []):
                if str(r["id"]) == ident:
                    return r
        raise ValueError("Release not found in the cache. Refresh the release list.")

    def save_release(self, r):
        if r.get("tracks_loaded"):
            self.store.save_preferences(
                f"tag-review:{self.market}:{r['id']}",
                dict(r, details_checked_at=time.time()),
            )
        with self.store.connect() as db:
            for c in db.execute(
                "SELECT artist_id,payload FROM catalogue WHERE market=?", (self.market,)
            ).fetchall():
                payload = json.loads(c[1])
                changed = False
                for i, release in enumerate(payload.get("releases", [])):
                    if str(release["id"]) == str(r["id"]):
                        payload["releases"][i] = dict(release, **r)
                        changed = True
                if changed:
                    db.execute(
                        "UPDATE catalogue SET payload=? WHERE artist_id=? AND market=?",
                        (json.dumps(payload), c[0], self.market),
                    )
            q = db.execute(
                "SELECT payload FROM queue WHERE id=?", (str(r["id"]),)
            ).fetchone()
            if q:
                original = json.loads(q[0])
                db.execute(
                    "UPDATE queue SET payload=? WHERE id=?",
                    (json.dumps(dict(original, **r)), str(r["id"])),
                )

    def view(self):
        key = ("catalogue", self.revision)
        result = self._views.get(key)
        if result is None:
            from .view_data import build_view

            result = build_view(self.store, self.market)
            self._views[key] = result
        return result

    @staticmethod
    def file_row(r):
        tags = r.get("tags", {})
        ids = r.get("linked_ids") or {}
        choice = r.get("catalogue_choice") or {}
        changes = r.get("changes") or {}
        name = first(tags, "title") or Path(r.get("path", "")).name
        from .release_matching import position, total

        disc, track = position(r)
        disc_total = total(r, "disc")
        track_total = total(r, "track")
        position_text = f"Disc {disc:02d}/{disc_total or '?'} · Track {track:02d}/{track_total or '?'}"
        return dict(
            id=r.get("path", r.get("folder", "")),
            artist=first(tags, "albumartist"),
            release=first(tags, "album"),
            title=name,
            path=r.get("path", r.get("folder", "")),
            position=position_text,
            status="Linked"
            if ids.get("track_id")
            else "Needs choice"
            if r.get("catalogue_options")
            else "Unlinked",
            evidence=r.get("blocked")
            or r.get("catalogue_note")
            or "; ".join(r.get("issues", [])),
            affected=bool(
                changes
                or r.get("artwork_change")
                or r.get("target", r.get("path")) != r.get("path")
                or r.get("superseded_by")
            ),
            target=r.get("target", r.get("path")),
            changes="; ".join(
                f"{k}: {', '.join(tags.get(k, [])) or 'missing'} → {', '.join(v) or 'remove'}"
                for k, v in changes.items()
            ),
            bpm=first(tags, "bpm"),
            key=first(tags, "initialkey") or first(tags, "key"),
            candidates=len(r.get("catalogue_options", [])),
            online_id=ids.get("album_id", ""),
        )

    def active_plan(self, a):
        route = a.get("route")
        if route not in ("correct", "organise", "metadata", "artwork"):
            return None
        root = self.root(a.get("root"))
        operation = (
            a.get("action", "dates" if route == "correct" else "organise")
            if route in ("correct", "organise")
            else route
        )
        p = self.plans.get(a.get("preview_id") or self.latest.get((root, operation)))
        return p if p and p["root"] == root and p["operation"] == operation else None

    def dataset(self, a):
        route = a["route"]
        root = (
            self.root(a.get("root"))
            if a.get("root") or self.store.rows("SELECT root FROM roots")
            else None
        )
        if route in ("files", "links", "correct", "organise", "metadata", "artwork"):
            if not root:
                return []
            rows = self.snapshot(root)
            if route in ("correct", "organise"):
                action = a.get("action", "dates" if route == "correct" else "organise")
                key = (root, action, self.revision)
                if key not in self._views:
                    from .library_workflows import workflow_plan

                    mode, flags = LOCAL_ACTIONS[action]
                    self._views[key] = workflow_plan(rows, mode, **flags)
                rows = self._views[key]
            plan = self.active_plan(a)
            if plan and plan["root"] == root:
                rows = plan["rows"]
            output = [self.file_row(r) for r in rows]
            ignored = self.store.ignored_local_files()
            for r in output:
                r["ignored"] = r["id"] in ignored
            return output
        if route == "artists":
            result = []
            view = self.view()
            mappings = view["mappings"]
            reviews = view["reviews"]
            for name, tracks in view["artists"].items():
                m = mappings.get(name, {})
                result.append(
                    dict(
                        id=name,
                        artist=name,
                        tracks=len(tracks),
                        release=len({t.get("album") for t in tracks}),
                        status=m.get("status", "Unresolved"),
                        online_id=m.get("tidal_id", ""),
                        evidence=m.get("evidence", ""),
                    )
                )
            return result
        if route in ("missing", "queue", "downloaded"):
            if route == "missing":
                from .view_data import filter_coverage

                view = self.view()
                decisions = {q["id"]: q["decision"] for q in view["queue"]}
                filters = (
                    a.get("timeline", "Newer than newest owned"),
                    "",
                    a.get("status", "All statuses"),
                    a.get("type", "All types"),
                    a.get("copyright", "All copyrights"),
                    a.get("recommendation", "All recommendations"),
                )
                items, _, _ = filter_coverage(view, decisions, filters, demo=self.demo)
                return [self.release_row(i["release"], i["state"], i) for i in items]
            return [
                self.release_row(
                    json.loads(q["payload"]),
                    q["decision"],
                    dict(approved=bool(q["approved"])),
                )
                for q in self.store.rows(
                    "SELECT * FROM queue WHERE decision=? ORDER BY updated DESC",
                    ("queued" if route == "queue" else "downloaded",),
                )
            ]
        if route == "favourites":
            from .favourites_view import favourite_rows

            fav = []
            for r in self.store.rows("SELECT payload FROM favourite_artists"):
                fav.extend(json.loads(r["payload"]))
            data = favourite_rows(
                fav, self.store.artists(), self.store.linked_mappings()
            )
            return [
                dict(id=r[0], artist=r[0], status=r[1], tracks=r[2], online_id=r[3])
                for r in data
            ]
        if route == "mqa":
            if not root:
                return []
            from .mqa_audit import cached_audit

            indexed = {r["path"]: r.get("tags", {}) for r in self.snapshot(root)}
            return [
                dict(
                    id=r["path"],
                    path=r["path"],
                    artist=first(indexed.get(r["path"], {}), "albumartist"),
                    release=first(indexed.get(r["path"], {}), "album"),
                    title=first(indexed.get(r["path"], {}), "title")
                    or Path(r["path"]).name,
                    status=r.get("status"),
                    evidence=r.get("evidence"),
                    target="Queue replacement" if r.get("detected") else "No change",
                    affected=r.get("detected"),
                )
                for r in cached_audit(self.store, root)
            ]
        if route in ("local", "online"):
            if not root:
                return []
            from .optimizations import load_optimization_results

            plans = (
                load_optimization_results(
                    self.store,
                    root,
                    self.market,
                    "local" if route == "local" else "remote",
                    allow_stale=True,
                )
                or []
            )
            return [
                dict(
                    id=p["folder"],
                    path=p["folder"],
                    artist=p["release"].get("artist"),
                    release=Path(p["folder"]).name,
                    target=p.get("target_folder") or p["release"].get("title"),
                    online_id=p["release"]["id"],
                    status="Remove duplicates"
                    if p["kind"] == "local"
                    else "Download replacement",
                    tracks=p["duplicates"],
                    gained=p["gained"],
                    bytes=p["recoverable_bytes"],
                    evidence=f"All {p['duplicates']} recordings have a verified replacement",
                    affected=True,
                )
                for p in plans
            ]
        return []

    def release_row(self, r, status, extra=None):
        extra = extra or {}
        selection = r.get("selected_tracks")
        approved = extra.get("approved", False)
        return dict(
            id=str(r["id"]),
            downloaded_files=r.get("downloaded_files", []),
            artist=r.get("artist", ""),
            release=r.get("title", ""),
            date=r.get("date", ""),
            type=r.get("type", ""),
            tracks=r.get("track_count", 0),
            status=status,
            online_id=str(r["id"]),
            recommendation=extra.get("recommendation", {}).get("badge", ""),
            evidence=extra.get("recommendation", {}).get("evidence", []),
            expanded_available=bool(r.get("tracks_loaded")),
            available=r.get("available"),
            approved=approved,
            selected=None
            if selection is None and approved
            else [str(t["id"]) for t in selection or []]
            if approved
            else [],
            children=[
                dict(
                    id=str(t["id"]),
                    title=t.get("title", ""),
                    position=f"{t.get('disc_number', 1)} · {t.get('track_number', '')}",
                    duration=t.get("duration"),
                    isrc=t.get("isrc", ""),
                    bpm=t.get("bpm"),
                    key=t.get("key"),
                )
                for t in r.get("tracks", [])
            ],
        )

    def table(self, a):
        key = (
            "table",
            self.revision,
            json.dumps(
                {
                    k: v
                    for k, v in a.items()
                    if k
                    not in ("offset", "limit", "search", "sort", "direction", "filter")
                },
                sort_keys=True,
            ),
        )
        cached = self._views.get(key)
        if cached is None:
            cached = self.dataset(a)
            self._views[key] = cached
        rows = list(cached)
        query = str(a.get("search", "")).casefold()
        filter = a.get("filter", "all")
        if query:
            rows = [
                r
                for r in rows
                if query
                in " ".join(
                    str(r.get(k, ""))
                    for k in (
                        "artist",
                        "release",
                        "title",
                        "path",
                        "evidence",
                        "online_id",
                    )
                ).casefold()
            ]
        if a.get("route") == "missing" and filter in (
            "Missing release",
            "Owned partial",
            "Owned complete",
            "Queued",
            "Ignored",
            "Unavailable",
        ):
            rows = [r for r in rows if r["status"] == filter]
        if filter == "affected":
            rows = [r for r in rows if r.get("affected")]
        if filter == "unlinked":
            rows = [
                r for r in rows if r.get("status") != "Linked" and not r.get("ignored")
            ]
        if filter == "choice":
            rows = [
                r
                for r in rows
                if r.get("status") != "Linked"
                and r.get("candidates")
                and not r.get("ignored")
            ]
        if filter == "linked":
            rows = [r for r in rows if r.get("status") == "Linked"]
        if filter == "ignored":
            rows = [r for r in rows if r.get("ignored")]
        if filter == "unresolved":
            rows = [r for r in rows if r.get("status") not in ("confirmed", "auto")]
        if filter in ("Missing locally", "In library", "Local only"):
            rows = [r for r in rows if r.get("status") == filter]
        sort = a.get("sort", "artist")
        numeric = all(isinstance(x.get(sort), (int, float)) for x in rows)
        rows.sort(
            key=lambda r: (
                (r.get(sort, 0),)
                if numeric
                else (r.get(sort) is None, str(r.get(sort, "")).casefold())
            ),
            reverse=a.get("direction") == "desc",
        )
        offset = max(0, int(a.get("offset", 0)))
        limit = max(1, min(250, int(a.get("limit", 100))))
        return clean(
            {
                "rows": rows[offset : offset + limit],
                "total": len(rows),
                "offset": offset,
                "revision": self.revision,
                "preview_id": (self.active_plan(a) or {}).get("id")
                if self.store.rows("SELECT root FROM roots")
                else None,
            }
        )

    def detail(self, a):
        if a.get("release_id"):
            release = self.release(a["release_id"])
            queued = self.store.rows(
                "SELECT payload FROM queue WHERE id=?", (str(a["release_id"]),)
            )
            if queued:
                release = dict(
                    release,
                    downloaded_files=json.loads(queued[0]["payload"]).get(
                        "downloaded_files", []
                    ),
                )
            return clean(self.release_row(release, "", {}))
        if a.get("artist"):
            review = self.store.rows(
                "SELECT * FROM match_reviews WHERE artist=?", (a["artist"],)
            )
            payload = json.loads(review[0]["payload"]) if review else {}
            payload["candidates"] = [
                dict(
                    c,
                    id=str(c.get("id") or c.get("artist", {}).get("id", "")),
                    name=c.get("name") or c.get("artist", {}).get("name", ""),
                )
                for c in payload.get("candidates", [])
            ]
            return clean(
                dict(
                    artist=a["artist"],
                    ids=[
                        str(m["tidal_id"])
                        for m in self.store.linked_mappings()
                        if m["artist"] == a["artist"]
                    ],
                    review=payload,
                )
            )
        row = self.paths(self.root(a.get("root")), [a["path"]])[0]
        result = copy.deepcopy(row)
        result["catalogue_options"] = [
            dict(
                o,
                choice_key=hashlib.sha256(
                    json.dumps(o, sort_keys=True).encode()
                ).hexdigest(),
            )
            for o in result.get("catalogue_options", [])
        ]
        return clean(dict(result, local_position=self.file_row(row)["position"]))
