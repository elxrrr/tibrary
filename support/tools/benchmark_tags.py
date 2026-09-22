"""Read-only parser comparison; optional writes are always disposable copies."""

import argparse, json, os, statistics, time, tempfile, shutil
from pathlib import Path
from mutagen.flac import FLAC as Mutagen
from library_manager.tag_io import FLAC as Native


def run(files, reader, repeats=3):
    times = []
    for _ in range(repeats):
        start = time.perf_counter()
        for path in files:
            reader(path)
        times.append(time.perf_counter() - start)
    return statistics.median(times)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--limit", type=int, default=100)
    args = parser.parse_args()
    files = []
    for directory, dirs, names in os.walk(args.root, followlinks=False):
        dirs[:] = [d for d in dirs if not Path(directory, d).is_symlink()]
        for name in names:
            path = Path(directory, name)
            if path.suffix.lower() == ".flac" and not path.is_symlink():
                files.append(path)
            if len(files) >= args.limit:
                break
        if len(files) >= args.limit:
            break
    if not files:
        raise SystemExit("No FLAC sample found")
    # Compatibility first, outside timed runs. Never change original metadata.
    for path in files:
        old = Mutagen(path)
        new = Native(path)
        if dict(old.tags or {}) != dict(new.tags):
            raise AssertionError(f"Tag mismatch: {path}")
        if [p.write() for p in old.pictures] != [p.write() for p in new.pictures]:
            raise AssertionError(f"Artwork mismatch: {path}")
    result = {
        "files": len(files),
        "read_mutagen_seconds": run(files, Mutagen),
        "read_lofty_seconds": run(files, Native),
    }
    result["scan_lofty_without_artwork_seconds"] = run(
        files, lambda path: Native(path, pictures=False)
    )
    result["read_speedup"] = (
        result["read_mutagen_seconds"] / result["read_lofty_seconds"]
    )
    writes = {}
    with tempfile.TemporaryDirectory(prefix="tibrary-native-benchmark-") as temp:
        for label, reader in [("mutagen", Mutagen), ("lofty", Native)]:
            samples = []
            for i, source in enumerate(files[:5]):
                target = Path(temp) / f"{label}-{i}.flac"
                shutil.copy2(source, target)
                start = time.perf_counter()
                audio = reader(target)
                audio["tibrary_benchmark"] = ["disposable copy"]
                audio.save()
                samples.append(time.perf_counter() - start)
            writes[label] = statistics.median(samples)
    result["write_copy_median_seconds"] = writes
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
