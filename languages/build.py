"""Build locked grammar sources; package final (already stripped/signed) bytes."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
from definitions import check

ROOT = Path(__file__).resolve().parent.parent
TARGETS = {
    'aarch64-apple-darwin': ('darwin-arm64', 'darwin', 'arm64'),
    'x86_64-apple-darwin': ('darwin-x64', 'darwin', 'x64'),
    'aarch64-unknown-linux-gnu': ('linux-arm64-gnu', 'linux', 'arm64'),
    'x86_64-unknown-linux-gnu': ('linux-x64-gnu', 'linux', 'x64'),
}


def read(path):
    return json.loads(path.read_text())


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument('action', choices=['build', 'package', 'check'])
    cli.add_argument('--target', choices=TARGETS)
    args = cli.parse_args()
    definitions = check()
    if args.action == 'check':
        print(f'{len(definitions)} optional grammar definitions validated')
        return
    if args.target is None:
        cli.error('--target is required for build/package')
    suffix, system, cpu = TARGETS[args.target]
    host = subprocess.check_output(['rustc', '-vV'], text=True).split('host: ')[1].splitlines()[0]
    if args.target != host and not (system == 'darwin' and host.endswith('-apple-darwin')):
        cli.error('build/package on the native target runner (macOS also supports cross-architecture builds)')
    architecture = ['-arch', 'arm64' if cpu == 'arm64' else 'x86_64'] if system == 'darwin' else []
    output = ROOT / 'target/languages' / args.target
    package = output / 'package'
    template = read(ROOT / 'languages/package.template.json')
    name = '@dev.fast/diffr-languages-extra-' + suffix
    if args.action == 'build':
        metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--all-features', '--format-version', '1'], cwd=ROOT))
        if package.exists():
            shutil.rmtree(package)
        package.mkdir(parents=True)
        (output / 'test-fixtures.txt').write_text(''.join(
            f"{d['fixture']['prefix']}:{d['fixture']['extension']}\n" for d in definitions))
        libraries = []
        for definition in definitions:
            crate = next(p for p in metadata['packages'] if p['name'] == definition['crate'] and p['version'] == definition['version'])
            crate_root = Path(crate['manifest_path']).parent
            assert read(crate_root / '.cargo_vcs_info.json')['git']['sha1'] == definition['revision']
            source = crate_root / definition['source']
            filename = definition['id'] + ('.dylib' if system == 'darwin' else '.so')
            sources = [source / 'parser.c']
            if (source / 'scanner.c').exists():
                sources.append(source / 'scanner.c')
            subprocess.run([os.environ.get('CC', 'cc'), *architecture, '-O2', '-std=c11', '-fPIC', '-I' + str(source), '-dynamiclib' if system == 'darwin' else '-shared', *map(str, sources), '-o', str(package / filename)], check=True)
            subprocess.run(['strip', '-x' if system == 'darwin' else '--strip-unneeded', str(package / filename)], check=True)
            abi = int(re.search(r'#define LANGUAGE_VERSION (\d+)', (source / 'parser.c').read_text())[1])
            libraries.append(dict(id=definition['id'], file=filename, symbol=definition['symbol'], abi=abi))
            shutil.copyfile(ROOT / 'languages' / definition['id'] / 'LICENSE', package / (definition['id'] + '.LICENSE'))
        write(package / 'manifest.json', dict(schema=1, pack='extra', version=template['version'], target=args.target, libraries=libraries))
        write(package / 'package.json', dict(template, name=name, os=[system], cpu=[cpu], **({'libc': ['glibc']} if system == 'linux' else {})))
    else:
        manifest = read(package / 'manifest.json')
        assert manifest['target'] == args.target and manifest['version'] == template['version']
        expected = {(d['id'], d['symbol']) for d in definitions}
        assert {(d['id'], d['symbol']) for d in manifest['libraries']} == expected, 'stale package: rebuild the language libraries'
        expected_files = {'manifest.json', 'package.json'}
        for library in manifest['libraries']:
            expected_files.update([library['file'], library['id'] + '.LICENSE'])
        assert {p.name for p in package.iterdir()} == expected_files, 'unexpected or missing package files'
        files = {p.name: dict(size=p.stat().st_size, sha256=digest(p.read_bytes())) for p in sorted(package.iterdir())}
        archive = output / (name.split('/')[1] + '-' + template['version'] + '.tgz')
        with archive.open('wb') as raw:
            with gzip.GzipFile(fileobj=raw, mode='wb', mtime=0, filename='') as compressed:
                with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as tar:
                    for filename in files:
                        data = (package / filename).read_bytes()
                        entry = tarfile.TarInfo('package/' + filename)
                        entry.size, entry.mode = len(data), 0o644
                        tar.addfile(entry, io.BytesIO(data))
        entry = dict(manifest, name=name, url=f'https://registry.npmjs.org/{name}/-/{archive.name}', size=archive.stat().st_size, sha256=digest(archive.read_bytes()), files=files)
        write(output / 'catalog-entry.json', entry)
        print(archive)


if __name__ == '__main__':
    main()
