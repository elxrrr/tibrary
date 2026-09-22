"""Build the macOS app with a relocatable, private Python runtime."""

import json, os, platform, shutil, subprocess, sys, tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DESKTOP = ROOT / "desktop"
TAURI = DESKTOP / "src-tauri"


def run(*args, **kw):
    subprocess.run(list(map(str, args)), check=True, cwd=ROOT, **kw)


def main():
    if sys.platform != "darwin":
        raise SystemExit("Run this macOS build on a Mac.")
    uv = ROOT / ".venv/bin/uv"
    if not uv.exists():
        run(sys.executable, "-m", "pip", "install", "uv==0.12.17")
    os.environ["PATH"] = (
        str(Path.home() / ".cargo/bin") + os.pathsep + os.environ["PATH"]
    )
    native = ROOT / "native/tibrary-tags"
    wheels = ROOT / "native/wheels"
    run(
        uv,
        "tool",
        "run",
        "--from",
        "maturin==1.15.0",
        "maturin",
        "build",
        "--release",
        "--locked",
        "--manifest-path",
        native / "Cargo.toml",
        "--out",
        wheels,
    )
    arch_tag = "arm64" if platform.machine() == "arm64" else "x86_64"
    native_version = tomllib.loads((native / "pyproject.toml").read_text())["project"]["version"]
    wheel = next(wheels.glob(f"tibrary_tags-{native_version}-cp312-abi3-*_{arch_tag}.whl"))
    runtime = TAURI / "runtime"
    home = runtime / "python"
    run(uv, "python", "install", "3.13.15", "--install-dir", home, "--no-bin")
    # Keep runtime aliases relocatable when promoting a staged build.
    for alias in home.iterdir():
        if alias.is_symlink():
            target = Path(alias.readlink()).name
            if (home / target).exists():
                alias.unlink()
                alias.symlink_to(target)
    python = next(home.glob("cpython-*/bin/python3"))
    run(
        uv,
        "pip",
        "install",
        "--break-system-packages",
        "--python",
        python,
        "--constraint",
        ROOT / "support/requirements-macos.lock",
        wheel,
        ROOT / "app",
        ROOT / "app/resources/python-tidal",
        ROOT / "app/resources/tidaler",
    )
    package = (
        next(home.glob("cpython-*/lib/python3.13/site-packages")) / "library_manager"
    )
    shutil.copytree(
        ROOT / "app/library_manager",
        package,
        dirs_exist_ok=True,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
    )
    run(python, package / "backend_probe.py")
    run(
        python,
        "-c",
        "import imageio_ffmpeg,subprocess; subprocess.run([imageio_ffmpeg.get_ffmpeg_exe(), '-version'],check=True,stdout=subprocess.DEVNULL)",
    )
    lock = subprocess.check_output(
        [str(uv), "pip", "freeze", "--python", str(python)], text=True
    )
    (runtime / "installed-packages.txt").write_text(lock)
    for d in list(runtime.rglob("__pycache__")):
        shutil.rmtree(d)
    # Preserve third-party source and licences alongside the binary distribution.
    for name in ("tidaler", "python-tidal"):
        source = ROOT / "app/resources" / name
        target = runtime / "sources" / name
        if target.exists():
            shutil.rmtree(target)
        shutil.copytree(
            source,
            target,
            ignore=shutil.ignore_patterns(
                ".git", ".venv", "__pycache__", "*.pyc", "dist", "build"
            ),
        )
    # Distribute the native adapter source and its dependency licence inventory.
    native_sources = runtime / "sources/native"
    shutil.copytree(
        native,
        native_sources / "tibrary-tags",
        dirs_exist_ok=True,
        ignore=shutil.ignore_patterns("target", "__pycache__"),
    )
    metadata = json.loads(
        subprocess.check_output(
            [
                "cargo",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--manifest-path",
                str(native / "Cargo.toml"),
            ],
            text=True,
        )
    )
    inventory = []
    for dependency in metadata["packages"]:
        directory = Path(dependency["manifest_path"]).parent
        name = dependency["name"] + "-" + dependency["version"]
        destination = native_sources / "licenses" / name
        destination.mkdir(parents=True, exist_ok=True)
        for license_file in directory.iterdir():
            if license_file.is_file() and license_file.name.upper().startswith(
                ("LICENSE", "COPYING", "NOTICE")
            ):
                shutil.copy2(license_file, destination / license_file.name)
        inventory.append(
            dict(
                name=dependency["name"],
                version=dependency["version"],
                license=dependency.get("license"),
            )
        )
    (native_sources / "dependencies.json").write_text(json.dumps(inventory, indent=2))
    arch = "aarch64" if platform.machine() == "arm64" else "x86_64"
    binary = TAURI / "binaries" / f"tibrary-service-{arch}-apple-darwin"
    binary.parent.mkdir(exist_ok=True)
    relative = str(python.relative_to(TAURI))
    binary.write_text(
        '#!/bin/sh\nset -eu\nHERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"\n'
        + f'PYTHON="$HERE/../Resources/{relative}"\n'
        + 'export TIBRARY_BUNDLED_PYTHON="$PYTHON"\nexport TIBRARY_RESOURCES="$HOME/Library/Application Support/Tibrary/streaming"\nexport PYTHONNOUSERSITE=1\nexport PYTHONDONTWRITEBYTECODE=1\nexec "$PYTHON" -m library_manager.sidecar "$@"\n'
    )
    binary.chmod(0o755)
    env = dict(
        os.environ,
        PATH=str(Path.home() / ".cargo/bin") + os.pathsep + os.environ["PATH"],
    )
    run("npm", "--prefix", DESKTOP, "ci", env=env)
    subprocess.run(
        ["npm", "run", "tauri", "--", "build", "--bundles", "app,dmg"],
        cwd=DESKTOP,
        env=env,
        check=True,
    )


if __name__ == "__main__":
    main()
