"""Shared definitions for packaging, smoke tests, and optional-parser checks."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def definitions(root=ROOT):
    return [json.loads(path.read_text()) for path in sorted((root / 'languages').glob('*/language.json'))]


def validate(items, metadata, root=ROOT):
    package = next(p for p in metadata['packages'] if p['name'] == 'diffr-cli')
    dependencies = {d['name']: d for d in package['dependencies']}
    registry = set(re.findall(r'(\w+)\s*=>\s*\("([^"]+)",\s*"([^"]+)"\)', (root / 'src/parse/optional.rs').read_text()))
    expected = {(d['variant'], d['id'], d['feature']) for d in items}
    assert registry == expected, 'optional Rust registry and language definitions differ'
    assert len({d['id'] for d in items}) == len(items), 'duplicate language ID'
    assert {d['crate'] for d in items} == {name for name, dep in dependencies.items() if dep['optional'] and name.startswith(('tree-sitter-', 'ts-parser-'))}, 'optional Cargo dependencies and definitions differ'
    assert {d['feature'] for d in items} == {name for name in package['features'] if name.startswith('lang-')}, 'language Cargo features and definitions differ'
    for definition in items:
        id, feature, crate = (definition[k] for k in ('id', 'feature', 'crate'))
        assert re.fullmatch(r'[a-z][a-z0-9-]*', id), f'{id}: invalid ID'
        assert definition['variant'].isidentifier(), f'{id}: invalid enum variant'
        assert package['features'][feature] == ['dep:' + crate], f'{id}: incorrect Cargo feature'
        assert feature in package['features']['all-languages'], f'{id}: missing from all-languages'
        assert dependencies[crate]['optional'], f'{id}: dependency must be optional'
        source = Path(definition['source'])
        assert not source.is_absolute() and '..' not in source.parts, f'{id}: invalid source path'
        assert re.fullmatch(r'[a-zA-Z_][a-zA-Z_0-9]*', definition['symbol']), f'{id}: invalid symbol'
        assert definition['smoke'].strip(), f'{id}: missing parse smoke input'
        for filename in ['LICENSE', 'highlights.scm']:
            assert (root / 'languages' / id / filename).is_file(), f'{id}: missing {filename}'
        fixture = definition['fixture']
        for side in (1, 2):
            path = root / 'sample_files' / f"{fixture['prefix']}_{side}.{fixture['extension']}"
            assert path.is_file(), f'{id}: missing fixture {path}'


def check(root=ROOT):
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], cwd=root))
    items = definitions(root)
    validate(items, metadata, root)
    return items
