"""Build test-only pinned CLIs, compare native/static output, exercise offline repair.

The tracked catalog is restored even on failure; no runtime trust override exists.
"""
import concurrent.futures
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'languages'))
from definitions import definitions
items = definitions()
entry_path = Path(sys.argv[1]).resolve()
entry = json.loads(entry_path.read_text())
archive = entry_path.parent / entry['url'].rsplit('/', 1)[1]
release = '--release' in sys.argv
profile = 'release' if release else 'debug'
catalog = root / 'languages/catalog.json'
saved = catalog.read_bytes()
output = root / 'target/languages' / entry['target'] / 'verification'
output.mkdir(exist_ok=True)
host = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]


def build(features, name):
    args = ['cargo', 'build', '--locked', '--bin', 'diffr']
    build_directory = root / 'target'
    if entry['target'] != host:
        args += ['--target', entry['target']]
        build_directory /= entry['target']
    if release:
        args += ['--release']
    if features:
        args += ['--features', features]
    subprocess.run(args, cwd=root, check=True)
    destination = output / name
    destination.unlink(missing_ok=True)
    shutil.copyfile(build_directory / profile / 'diffr', destination)
    destination.chmod(0o755)
    identity = os.environ.get('DIFFR_SIGN_IDENTITY')
    if identity:
        subprocess.run(['codesign', '--force', '--timestamp', '--options', 'runtime', '--sign', identity, '--entitlements', str(root / 'languages/tests/entitlements.plist'), str(destination)], check=True)
    return destination


try:
    catalog.write_text(json.dumps({'schema': 1, 'packages': [entry]}) + '\n')
    native = build(None, 'diffr-native')
    static = build('all-languages', 'diffr-static')
finally:
    catalog.write_bytes(saved)

with tempfile.TemporaryDirectory() as temporary:
    store = Path(temporary) / 'store'
    env = dict(os.environ, DIFFR_PARSER_DIR=str(store))

    def run(binary, *args, code=0):
        process = subprocess.run([str(binary), *map(str, args)], env=env, cwd=root, capture_output=True, text=True)
        assert process.returncode == code, (args, process.returncode, process.stdout, process.stderr)
        return [json.loads(line) for line in process.stdout.splitlines()]

    def compare_args(fixture, extension):
        return ['--format', 'ndjson', '--syntax', '--no-index', f'sample_files/{fixture}_1.{extension}', f'sample_files/{fixture}_2.{extension}']

    fixtures = [(d['fixture']['prefix'], d['fixture']['extension']) for d in items]
    first_id = items[0]['id']
    args = compare_args(*fixtures[0])
    missing = run(native, *args)
    assert next(e for e in missing if e['type'] == 'file')['diff']['stats']['fallback']['code'] == 'parser_not_installed'
    assert not store.exists(), 'ordinary diffs must not create/download packs'
    damaged = Path(temporary) / 'damaged.tgz'
    damaged.write_bytes(archive.read_bytes()[:-32])
    rejected = run(native, 'languages', 'install', 'extra', '--json', '--archive', damaged, code=2)
    assert rejected[0]['error']['code'] == 'language_install_failed'
    assert not (store / entry['target'] / 'extra' / entry['version']).exists()
    previous = entry_path.parent / 'previous'
    previous_tested = (previous / 'diffr').exists()
    if previous_tested:
        previous_entry = json.loads((previous / 'catalog-entry.json').read_text())
        previous_archive = previous / previous_entry['url'].rsplit('/', 1)[1]
        run(previous / 'diffr', 'languages', 'install', 'extra', '--json', '--archive', previous_archive)
    older = store / entry['target'] / 'extra' / '0.0.0'
    older.mkdir(parents=True)
    (older / 'sentinel').write_text('older CLI')
    installed = lambda: run(native, 'languages', 'install', 'extra', '--json', '--archive', archive)
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        list(executor.map(lambda _: installed(), range(4)))
    status = run(native, 'languages', 'list', '--json')[0]
    assert status['target'] == entry['target'], 'select packages using executable architecture'
    assert {l['id'] for l in status['languages']} == {d['id'] for d in items}
    assert all(l['availability'] == 'installed' for l in status['languages'])
    for fixture, extension in fixtures:
        args = compare_args(fixture, extension)
        expected = run(static, *args)
        actual = run(native, *args)
        # Run summaries include elapsed time; file and syntax events are deterministic.
        relevant = lambda events: [e for e in events if e['type'] in ('file', 'syntax')]
        assert relevant(actual) == relevant(expected), fixture
        assert not next(e for e in actual if e['type'] == 'file')['diff']['stats'].get('fallback'), fixture
        for flag in ['--dump-ts', '--dump-syntax']:
            path = f'sample_files/{fixture}_1.{extension}'
            a = subprocess.check_output([native, 'debug', flag, path], env=env, cwd=root)
            b = subprocess.check_output([static, 'debug', flag, path], env=env, cwd=root)
            assert a == b, (fixture, flag)
    for extension in ['f90', 'fs']:
        original = root / f'languages/tests/fixtures/scanner.{extension}'
        changed = Path(temporary) / f'changed.{extension}'
        changed.write_text(original.read_text().replace('2', '4'))
        args = ['--format', 'ndjson', '--syntax', '--no-index', str(original), str(changed)]
        assert relevant(run(native, *args)) == relevant(run(static, *args))
        tree = subprocess.check_output([native, 'debug', '--dump-ts', original], env=env, cwd=root)
        assert b'ERROR' not in tree and b'MISSING' not in tree, extension
    # Exercise synchronized first loads and repeated grammars in a single process.
    repository = Path(temporary) / 'repository'
    repository.mkdir()
    def git(*args):
        subprocess.run(['git', '-C', str(repository), *args], check=True, capture_output=True)
    git('init')
    for fixture, extension in fixtures:
        for index in range(12):
            (repository / f'{fixture}-{index}.{extension}').write_bytes((root / f'sample_files/{fixture}_1.{extension}').read_bytes())
    git('add', '.')
    git('-c', 'user.name=Pack test', '-c', 'user.email=pack@example.invalid', 'commit', '-m', 'fixtures')
    for fixture, extension in fixtures:
        for index in range(12):
            (repository / f'{fixture}-{index}.{extension}').write_bytes((root / f'sample_files/{fixture}_2.{extension}').read_bytes())
        before = str(root / f'sample_files/{fixture}_1.{extension}')
        for left, right in [(before, before), ('/dev/null', before), (before, '/dev/null')]:
            args = ['--format', 'ndjson', '--syntax', '--no-index', left, right]
            assert relevant(run(native, *args)) == relevant(run(static, *args))
    args = ['--repo', repository, '--format', 'ndjson', '--syntax', '--jobs', '8', 'HEAD']
    ordered = lambda events: sorted((json.dumps(e, sort_keys=True) for e in relevant(events)))
    start = time.perf_counter()
    batch = run(native, *args)
    batch_ms = (time.perf_counter() - start) * 1000
    assert ordered(batch) == ordered(run(static, *args))
    revision = store / entry['target'] / 'extra' / entry['version']
    library = revision / next(l['file'] for l in entry['libraries'] if l['id'] == first_id)
    library.write_bytes(b'broken')
    broken = run(native, *compare_args(*fixtures[0]))
    assert next(e for e in broken if e['type'] == 'file')['diff']['stats']['fallback']['code'] == 'parser_load_failed'
    assert all(l['availability'] == 'built_in' for l in run(static, 'languages', 'list', '--json')[0]['languages'])
    installed()
    assert library.stat().st_size == entry['files'][library.name]['size']
    installed()  # idempotent, offline
    timings = []
    for _ in range(10):
        start = time.perf_counter()
        run(native, *compare_args(*fixtures[0]))
        timings.append((time.perf_counter() - start) * 1000)
    print(json.dumps({'archive_bytes': archive.stat().st_size, 'native_executable_bytes': native.stat().st_size, 'static_executable_bytes': static.stat().st_size, 'first_diff_ms': timings[0], 'median_process_diff_ms': statistics.median(timings), 'mixed_file_count': len(fixtures) * 12, 'mixed_batch_diff_ms': batch_ms}, indent=2))
    assert (older / 'sentinel').read_text() == 'older CLI'
    if previous_tested:
        previous_status = run(previous / 'diffr', 'languages', 'list', '--json')[0]
        assert previous_status['revision'] == previous_entry['version'] != entry['version']
        assert all(l['availability'] == 'installed' for l in previous_status['languages'])
        run(previous / 'diffr', *compare_args('fortran', 'f90'))
    # Exercise the platform's default data directory, including macOS's spaced path.
    home = Path(temporary) / 'home'
    home.mkdir()
    env.pop('DIFFR_PARSER_DIR')
    env['HOME'] = str(home)
    env['XDG_DATA_HOME'] = str(home / 'data')
    env['XDG_CONFIG_HOME'] = str(home / 'config')
    installed()
    default_root = home / ('Library/Application Support' if sys.platform == 'darwin' else 'data') / 'diffr/parsers'
    assert (default_root / entry['target'] / 'extra' / entry['version'] / 'manifest.json').exists()
    run(native, *compare_args(*fixtures[0]))
    if sys.platform == 'darwin':
        # Match a browser-downloaded archive's quarantine attribute without weakening library validation.
        quarantined = Path(temporary) / 'quarantined.tgz'
        shutil.copyfile(archive, quarantined)
        subprocess.run(['/usr/bin/xattr', '-w', 'com.apple.quarantine', '0081;00000000;diffr-test;', str(quarantined)], check=True)
        env['DIFFR_PARSER_DIR'] = str(Path(temporary) / 'quarantine-store')
        run(native, 'languages', 'install', 'extra', '--json', '--archive', quarantined)
        run(native, *compare_args(*fixtures[0]))
    print(json.dumps({'previous_cli_coexistence': previous_tested, 'default_store': str(default_root), 'quarantined_archive': sys.platform == 'darwin'}))
    env['PATH'] = ''
    env['DIFFR_PARSER_DIR'] = str(Path(temporary) / 'fresh-store')
    run(native, 'languages', 'list', '--json')
    installed()
    run(native, *compare_args(*fixtures[0]))
    print('Native/static NDJSON, trees, syntax, concurrent install, corruption, repair and offline reuse passed.')
