#!/usr/bin/env python3
"""Build local viewer data from verified fixture blobs via the real Git-ref CLI."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).parent / 'data'
OUT.mkdir(exist_ok=True)
ENV = dict(os.environ, GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1',
           GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid',
           GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid',
           GIT_AUTHOR_DATE='2000-01-01T00:00:00Z', GIT_COMMITTER_DATE='2000-01-01T00:00:00Z')
ENV.pop('DFT_DBG_KEEP_UNCHANGED', None)

def git(repo, *args, input=None):
    return subprocess.check_output(['git', '-C', str(repo), *args], input=input, env=ENV).decode().strip()

# Build pinned comparisons and capture the CLI stream for the static viewer.
REPO = OUT / "workspace"
REPO.mkdir(exist_ok=True)
git(REPO, 'init', '-q')
sides = [[], []]
index = []
for directory in sorted((ROOT / 'examples/review/real').iterdir()):
    provenance = json.loads((directory / 'provenance.json').read_text())
    case = json.loads((directory / 'case.json').read_text())
    sources = provenance['sources']
    path = sources['rhs']['path'] or sources['lhs']['path']
    served_path = directory.name + "/" + path
    for i, side in enumerate(('lhs', 'rhs')):
        entry = sources[side]
        if entry['path'] is not None:
            data = (directory / entry['file']).read_bytes()
            blob = git(REPO, 'hash-object', '-w', '--stdin', input=data)
            sides[i].append((served_path, blob))
    with tempfile.TemporaryDirectory(prefix='difft-viewer-') as repo:
        git(repo, 'init', '-q')
        commits = []
        for side in ('lhs', 'rhs'):
            entry = sources[side]
            data = (directory / entry['file']).read_bytes()
            assert hashlib.sha256(data).hexdigest() == entry['sha256']
            git(repo, 'read-tree', '--empty')
            if entry['path'] is not None:
                blob = git(repo, 'hash-object', '-w', '--stdin', input=data)
                assert blob == entry['git_blob_sha']
                git(repo, 'update-index', '--add', '--cacheinfo', '100644', blob, path)
            else:
                assert not data
            tree = git(repo, 'write-tree')
            parent = ['-p', commits[0]] if commits else []
            commits.append(git(repo, 'commit-tree', tree, '-m', 'Pinned fixture', *parent))
        view = {}
        view['patch'] = git(repo, 'diff', '--no-ext-diff', '--no-textconv', '--no-color', '--diff-algorithm=myers', '-U3', *commits, '--', path)
        view['full_patch'] = git(repo, 'diff', '--no-ext-diff', '--no-textconv', '--no-color', '--diff-algorithm=myers', '-U999999', *commits, '--', path)
    meta = dict(id=directory.name, repo=provenance['repo'], pr=provenance['pr_number'],
                title=provenance['pr_title'], url=provenance['pr_url'], language=provenance['language'],
                path=path, summary=case['summary'])
    view['meta'] = meta
    view['request'] = {'files': {'paths': [served_path]}}
    (OUT / f'{directory.name}.json').write_text(json.dumps(view))
    index.append(meta)
    print(directory.name)
(OUT / 'index.json').write_text(json.dumps(index))

commits = []
for files in sides:
    git(REPO, 'read-tree', '--empty')
    for path, blob in files:
        git(REPO, 'update-index', '--add', '--cacheinfo', '100644', blob, path)
    tree = git(REPO, 'write-tree')
    parent = ['-p', commits[0]] if commits else []
    commits.append(git(REPO, 'commit-tree', tree, '-m', 'Fixture stream snapshot', *parent))
for meta in index:
    path = OUT / (meta['id'] + '.json')
    view = json.loads(path.read_text())
    paths = view['request']['files']['paths']
    stream_path = OUT / (meta['id'] + '.ndjson')
    with stream_path.open('wb') as output:
        subprocess.run([str(ROOT / 'target/debug/diffr'), '--repo', str(REPO),
                        *commits, '--format', 'ndjson-v1', '--', *paths],
                       stdout=output, env=ENV, check=True)
    view['request'] = 'data/' + stream_path.name
    path.write_text(json.dumps(view))
