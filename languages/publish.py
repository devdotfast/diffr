"""Publish exact prepared archives, verify registry bytes, then emit a catalog.

This command is used only by the protected publication workflow.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request
from build import TARGETS


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--publish', action='store_true')
    args = parser.parse_args()
    (args.directory / "catalog.json").unlink(missing_ok=True)
    entries = [json.loads(p.read_text()) for p in sorted(args.directory.rglob('catalog-entry.json'))]
    assert len(entries) == 4 and {e['target'] for e in entries} == set(TARGETS), 'need exactly four targets'
    assert len({e['version'] for e in entries}) == 1, 'mixed revisions'
    for entry in entries:
        archives = list(args.directory.rglob(entry['url'].rsplit('/', 1)[1]))
        assert len(archives) == 1, 'missing or duplicate archive'
        data = archives[0].read_bytes()
        assert len(data) == entry['size'] and sha(data) == entry['sha256']
        if args.publish:
            try:
                with urllib.request.urlopen(entry['url'], timeout=60) as response:
                    existing = response.read(entry['size'] + 1)
                assert len(existing) == entry['size'] and sha(existing) == entry['sha256'], 'immutable revision already has different bytes'
            except urllib.error.HTTPError as error:
                if error.code != 404:
                    raise
                subprocess.run(['npm', 'publish', str(archives[0]), '--access', 'public', '--ignore-scripts'], check=True)
        # Never advance the catalog based on local artifacts alone.
        for attempt in range(12):
            try:
                with urllib.request.urlopen(entry['url'], timeout=60) as response:
                    downloaded = response.read(entry['size'] + 1)
                assert len(downloaded) == entry['size'] and sha(downloaded) == entry['sha256'], 'registry byte mismatch'
                break
            except urllib.error.HTTPError as error:
                if error.code != 404 or attempt == 11:
                    raise
                time.sleep(5)
    (args.directory / 'catalog.json').write_text(json.dumps({'schema': 1, 'packages': entries}, indent=2) + '\n')


if __name__ == '__main__':
    main()
