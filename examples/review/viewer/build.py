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

index = []
for directory in sorted((ROOT / 'examples/review/real').iterdir()):
    provenance = json.loads((directory / 'provenance.json').read_text())
    case = json.loads((directory / 'case.json').read_text())
    sources = provenance['sources']
    path = sources['rhs']['path'] or sources['lhs']['path']
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
        output = subprocess.check_output([str(ROOT / 'target/debug/difft'), 'review', '--repo', repo,
                    '--base', commits[0], '--head', commits[1], '--path', path, '--format', 'viewer'], env=ENV)
        view = json.loads(output)
        view['patch'] = git(repo, 'diff', '--no-ext-diff', '--no-textconv', '--no-color', '--diff-algorithm=myers', '-U3', *commits, '--', path)
        view['full_patch'] = git(repo, 'diff', '--no-ext-diff', '--no-textconv', '--no-color', '--diff-algorithm=myers', '-U999999', *commits, '--', path)
    meta = dict(id=directory.name, repo=provenance['repo'], pr=provenance['pr_number'],
                title=provenance['pr_title'], url=provenance['pr_url'], language=provenance['language'],
                path=path, summary=case['summary'])
    view['meta'] = meta
    (OUT / f'{directory.name}.json').write_text(json.dumps(view))
    index.append(meta)
    print(directory.name, len(view['domain']['lhs_folds']) + len(view['domain']['rhs_folds']), 'folds')
(OUT / 'index.json').write_text(json.dumps(index))
