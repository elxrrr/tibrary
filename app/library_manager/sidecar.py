"""Private newline-delimited JSON transport. No HTTP listener or remote access."""

import argparse
import concurrent.futures
import json
import os
import sys
import threading
from pathlib import Path
from .core import Store
from .desktop_service import DesktopService, clean


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db")
    parser.add_argument("--demo", action="store_true")
    args = parser.parse_args()
    folder = (
        Path.home() / "Library/Application Support/Tibrary"
        if sys.platform == "darwin"
        else Path.home() / ".local/share/tibrary"
    )
    store = Store(
        Path(args.db)
        if args.db
        else folder / ("tauri-demo.sqlite3" if args.demo else "library.sqlite3")
    )
    lockfile = open(str(store.path) + ".desktop.lock", "a+")
    if os.name == "posix":
        import fcntl

        try:
            fcntl.flock(lockfile, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit("Tibrary is already using this database")
    from .diagnostics import install

    install(store.path.parent / "logs")
    if args.demo:
        from .demo import seed

        seed(store)
    output = sys.stdout
    write_lock = threading.Lock()

    def emit(value):
        try:
            text = json.dumps(clean(value), ensure_ascii=False, allow_nan=False)
            with write_lock:
                output.write(text + "\n")
                output.flush()
        except (BrokenPipeError, OSError):
            service.cancel_event.set()

    service = DesktopService(store, emit, demo=args.demo)
    # Keep incidental upstream stdout away from the protocol stream.
    sys.stdout = sys.stderr

    def handle(message):
        ident = message.get("id")
        try:
            result = service.dispatch(message["method"], message.get("args", {}))
            emit({"id": ident, "result": result})
        except Exception as exc:
            emit({"id": ident, "error": service.credentials.redact(str(exc))})

    with concurrent.futures.ThreadPoolExecutor(
        max_workers=1, thread_name_prefix="desktop-requests"
    ) as pool:
        service.start("startup", {})
        emit({"event": "ready"})
        for line in sys.stdin:
            if len(line) > 8 * 1024 * 1024:
                emit({"error": "Request too large"})
                continue
            try:
                message = json.loads(line)
            except ValueError:
                emit({"error": "Invalid JSON"})
                continue
            if not isinstance(message, dict):
                continue
            # Cancellation and auth must not wait behind a large table/read operation.
            if message.get("method") in ("job.cancel", "auth.reply", "shutdown"):
                handle(message)
            else:
                pool.submit(handle, message)
        service.cancel_event.set()
    if service.worker:
        service.worker.join()


if __name__ == "__main__":
    main()
