"""Isolated, pinned upstream builds with an atomic activation and rollback."""
import json
import os
import shutil
import subprocess
import time
import uuid
import re
from .tidal import CatalogueError
from pathlib import Path

RESOURCES=Path(__file__).resolve().parents[1]/'resources'
UPSTREAMS={'tidaler':'https://github.com/maya-doshi/tidaler.git',
           'python-tidal':'https://github.com/EbbLabs/python-tidal.git'}


def active_runtime(resources=RESOURCES):
    manifest=resources/'backends'/'active.json'
    if manifest.exists():
        data=json.loads(manifest.read_text())
        candidate=(resources/'backends'/data['build']).resolve()
        if not candidate.is_relative_to((resources/'backends').resolve()):raise ValueError('Invalid backend installation path')
        python=candidate/'.venv/bin/python'
    else:python=resources/'tidaler/.venv/bin/python'
    if not python.is_file():raise ValueError('Downloader is not installed. Run app/tools/Setup downloads.command or update it in Settings.')
    return python


def versions(resources=RESOURCES):
    manifest=resources/'backends/active.json'
    if manifest.exists():
        data=json.loads(manifest.read_text());return ' · '.join(f'{name}: {commit[:12]}' for name,commit in data['commits'].items())
    values=[]
    for name in UPSTREAMS:
        try:
            result=subprocess.run(['git','-C',str(resources/name),'rev-parse','--short=12','HEAD'],capture_output=True,text=True,timeout=5,check=True)
            values.append(f'{name} source: {result.stdout.strip()}')
        except (OSError,subprocess.SubprocessError):values.append(name+': unavailable')
    return ' · '.join(values)+' · bundled runtime'


def check_upstream_updates(resources=RESOURCES, timeout=10):
    manifest = resources / 'backends' / 'active.json'
    active_commits = {}
    if manifest.exists():
        try:
            active_commits = json.loads(manifest.read_text()).get('commits', {})
        except Exception:
            pass

    results = {}
    updates_available = False
    for name, url in UPSTREAMS.items():
        curr = active_commits.get(name)
        if not curr:
            try:
                res = subprocess.run(['git', '-C', str(resources / name), 'rev-parse', 'HEAD'],
                                     capture_output=True, text=True, timeout=5, check=True)
                curr = res.stdout.strip()
            except Exception:
                curr = None

        remote = None
        try:
            res = subprocess.run(['git', 'ls-remote', url, 'HEAD'],
                                 capture_output=True, text=True, timeout=timeout, check=True)
            if res.stdout.strip():
                remote = res.stdout.strip().split()[0]
        except Exception:
            remote = None

        has_update = bool(curr and remote and curr != remote)
        if has_update:
            updates_available = True
        results[name] = {
            'current': (curr[:12] if curr else 'unknown'),
            'remote': (remote[:12] if remote else 'unavailable'),
            'has_update': has_update,
            'current_full': curr,
            'remote_full': remote,
        }

    summary_parts = []
    for name, info in results.items():
        if info['has_update']:
            summary_parts.append(f"{name}: update available ({info['current']} → {info['remote']})")
        else:
            summary_parts.append(f"{name}: up to date ({info['current']})")

    return {
        'updates_available': updates_available,
        'details': results,
        'summary': ' · '.join(summary_parts),
        'version_string': versions(resources)
    }


def run_command(command,cancel,timeout=900):
    process=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    started=time.monotonic()
    try:
        while True:
            if cancel():raise CatalogueError('Backend update cancelled; the active installation is unchanged.')
            if time.monotonic()-started>timeout:raise CatalogueError('Backend update timed out; the active installation is unchanged.')
            try:output,_=process.communicate(timeout=.2);break
            except subprocess.TimeoutExpired:continue
        if process.returncode:raise CatalogueError('Backend build failed; the active installation is unchanged. '+re.sub(r'https?://\S+','[package source]',output[-1800:]))
        return output.strip()
    finally:
        if process.poll() is None:
            process.terminate()
            try:process.communicate(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.communicate()


def activate(resources,manifest):
    home=resources/'backends';home.mkdir(parents=True,exist_ok=True)
    active=home/'active.json';temporary=home/'active.json.tmp'
    if active.exists():
        previous=home/'previous.json.tmp';previous.write_bytes(active.read_bytes());os.replace(previous,home/'previous.json')
    else:(home/'previous.json').write_text(json.dumps({'bundled':True}))
    temporary.write_text(json.dumps(manifest,indent=2));os.replace(temporary,active)


def update(cancel=lambda:False,progress=lambda message:None,resources=RESOURCES,runner=run_command):
    home=resources/'backends';home.mkdir(parents=True,exist_ok=True)
    build=home/('build-'+uuid.uuid4().hex);build.mkdir()
    activated=False
    try:
        commits={}
        for name,url in UPSTREAMS.items():
            progress(f'Fetching current {name} source into an isolated build')
            runner(['git','clone','--depth','1','--',url,str(build/name)],cancel)
            commits[name]=runner(['git','-C',str(build/name),'rev-parse','HEAD'],cancel)
        bootstrap=resources/'tidaler/.venv/bin/python'
        if not bootstrap.is_file():
            bootstrap=Path(shutil.which('python3.13') or '/opt/homebrew/bin/python3.13')
        progress('Building the separate Python runtime; the current downloader remains available')
        runner([str(bootstrap),'-m','venv',str(build/'.venv')],cancel)
        python=build/'.venv/bin/python'
        runner([str(python),'-m','pip','install',str(build/'python-tidal'),str(build/'tidaler')],cancel)
        progress('Checking dependency compatibility and the downloader adapter')
        runner([str(python),'-m','pip','check'],cancel)
        probe=Path(__file__).with_name('backend_probe.py')
        runner([str(python),str(probe)],cancel)
        if cancel():raise CatalogueError('Backend update cancelled; current installation retained.')
        activate(resources,dict(build=build.name,commits=commits));activated=True
        return 'Backend updated and compatibility checks passed. Previous installation retained for rollback.'
    finally:
        if not activated:shutil.rmtree(build,ignore_errors=True)


def rollback(resources=RESOURCES):
    home=resources/'backends';previous=home/'previous.json';active=home/'active.json'
    if not previous.exists():return 'No previous backend installation is saved.'
    data=json.loads(previous.read_text())
    if data.get('bundled'):
        if not (resources/'tidaler/.venv/bin/python').is_file():raise ValueError('Bundled runtime is unavailable')
        active.unlink(missing_ok=True);previous.unlink()
    else:
        target=(home/data['build']).resolve()
        if not target.is_relative_to(home.resolve()) or not (target/'.venv/bin/python').is_file():raise ValueError('Previous runtime is unavailable')
        activate(resources,data)
    return 'Previous backend restored.'
