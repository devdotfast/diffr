"""Shared definitions for packaging, smoke tests, and optional-parser checks."""
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def definitions(root=ROOT):
    return json.loads((root / 'languages/extra.json').read_text())


def validate(items, metadata, root=ROOT):
    package = next(p for p in metadata['packages'] if p['name'] == 'diffr-cli')
    dependencies = {d['name']: d for d in package['dependencies']}
    builtins = set(re.findall(r'^        (\w+) => \{', (root / 'src/parse/tree_sitter_parser.rs').read_text(), re.MULTILINE))
    assert len({d['id'] for d in items}) == len(items), 'duplicate language ID'
    assert len({d['variant'] for d in items}) == len(items), 'duplicate language variant'
    assert {d['crate'] for d in items} == {name for name, dep in dependencies.items() if dep['optional'] and name.startswith(('tree-sitter-', 'ts-parser-'))}, 'optional Cargo dependencies and definitions differ'
    assert {d['feature'] for d in items} == {name for name in package['features'] if name.startswith('lang-')}, 'language Cargo features and definitions differ'
    for definition in items:
        id, feature, crate = (definition[k] for k in ('id', 'feature', 'crate'))
        assert re.fullmatch(r'[a-z][a-z0-9-]*', id), f'{id}: invalid ID'
        assert definition['variant'].isidentifier(), f'{id}: invalid enum variant'
        assert definition['variant'] not in builtins, f'{id}: remove duplicate built-in config'
        assert package['features'][feature] == ['dep:' + crate], f'{id}: incorrect Cargo feature'
        assert feature in package['features']['all-languages'], f'{id}: missing from all-languages'
        assert dependencies[crate]['optional'], f'{id}: dependency must be optional'
        source = Path(definition['source'])
        assert not source.is_absolute() and '..' not in source.parts, f'{id}: invalid source path'
        assert re.fullmatch(r'[a-zA-Z_][a-zA-Z_0-9]*', definition['symbol']), f'{id}: invalid symbol'
        assert definition['smoke'].strip(), f'{id}: missing parse smoke input'
        assert re.fullmatch(r'\w+(::\w+)+', definition['parser']), f'{id}: invalid parser constant'
        licenses = [definition['license']] + [q['license'] for q in definition['highlights'] if 'crate' in q and q['crate'] != crate]
        for license in licenses:
            path = Path(license['path'])
            assert not path.is_absolute() and '..' not in path.parts, f'{id}: invalid license path'
            assert re.fullmatch(r'[0-9a-f]{64}', license['sha256']), f'{id}: invalid license hash'
        for query in definition['highlights']:
            if 'local' in query:
                relative = Path(query['local'])
                assert not relative.is_absolute() and '..' not in relative.parts, f'{id}: invalid local query path'
                path = root / relative
                assert path.is_file(), f'{id}: missing local query'
            else:
                assert query['crate'] in dependencies, f'{id}: unknown query crate'
                assert re.fullmatch(r'\w+(::\w+)*', query['constant']), f'{id}: invalid query constant'
                path = Path(query['path'])
                assert not path.is_absolute() and '..' not in path.parts, f'{id}: invalid query path'
        fixture = definition['fixture']
        for side in (1, 2):
            path = root / 'sample_files' / f"{fixture['prefix']}_{side}.{fixture['extension']}"
            assert path.is_file(), f'{id}: missing fixture {path}'


def check(root=ROOT):
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], cwd=root))
    items = definitions(root)
    validate(items, metadata, root)
    return items
