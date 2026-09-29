"""Collect licenses from locked Cargo sources or their exact upstream revision."""
import hashlib
import json
from pathlib import Path
import re
from urllib.request import urlopen


def collect(crate, spec, cache):
    root = Path(crate['manifest_path']).parent
    path = Path(spec['path'])
    assert not path.is_absolute() and '..' not in path.parts, 'invalid license path'
    fingerprint = spec['sha256']
    assert re.fullmatch(r'[0-9a-f]{64}', fingerprint), 'invalid license hash'
    cached = cache / fingerprint
    if (root / path).is_file():
        data = (root / path).read_bytes()
    elif cached.is_file():
        data = cached.read_bytes()
    else:
        revision = json.loads((root / '.cargo_vcs_info.json').read_text())['git']['sha1']
        assert re.fullmatch(r'[0-9a-f]{40}', revision), 'invalid upstream revision'
        repository = crate['repository'].removesuffix('.git').removesuffix('/')
        assert re.fullmatch(r'https://github.com/[\w.-]+/[\w.-]+', repository), 'unsupported license repository'
        url = repository.replace('https://github.com/', 'https://raw.githubusercontent.com/') + '/' + revision + '/' + path.as_posix()
        with urlopen(url, timeout=30) as response:
            data = response.read(1024 * 1024 + 1)
    assert len(data) <= 1024 * 1024, 'license exceeds size limit'
    assert hashlib.sha256(data).hexdigest() == fingerprint, f"{crate['name']}: license hash mismatch"
    cache.mkdir(parents=True, exist_ok=True)
    cached.write_bytes(data)
    return data


def package_licenses(definitions, metadata, package, cache):
    crates = {p['name']: p for p in metadata['packages']}
    for definition in definitions:
        (package / (definition['id'] + '.LICENSE')).write_bytes(
            collect(crates[definition['crate']], definition['license'], cache))
        for query in definition['highlights']:
            if 'crate' in query and query['crate'] != definition['crate']:
                (package / (query['crate'] + '.LICENSE')).write_bytes(
                    collect(crates[query['crate']], query['license'], cache))
