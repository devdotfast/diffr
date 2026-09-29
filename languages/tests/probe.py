"""Compile a small native host and parse with every pack library.

Set DIFFR_SIGN_IDENTITY on macOS to test hardened runtime library validation.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
package = Path(sys.argv[1]).resolve()
metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--format-version', '1'], cwd=root))
runtime = Path(next(p for p in metadata['packages'] if p['name'] == 'tree-sitter')['manifest_path']).parent
probe = package.parent / 'probe'
manifest = json.loads((package / 'manifest.json').read_text())
architecture = ['-arch', 'arm64' if manifest['target'].startswith('aarch64-') else 'x86_64'] if sys.platform == 'darwin' else []
subprocess.run(['cc', *architecture, '-O2', '-D_DEFAULT_SOURCE', '-I' + str(runtime / 'include'), '-I' + str(runtime / 'src'), str(runtime / 'src/lib.c'), str(root / 'languages/tests/probe.c'), '-o', str(probe), *(['-ldl'] if sys.platform != 'darwin' else [])], check=True)
identity = os.environ.get('DIFFR_SIGN_IDENTITY')
if identity:
    for path in [probe, *[package / entry['file'] for entry in manifest['libraries']]]:
        subprocess.run(['codesign', '--force', '--timestamp', '--options', 'runtime', '--sign', identity, str(path)], check=True)
        subprocess.run(['codesign', '--verify', '--strict', str(path)], check=True)
sys.path.insert(0, str(root / 'languages'))
from definitions import definitions
fixtures = {definition['id']: definition['smoke'] for definition in definitions()}
for entry in manifest['libraries']:
    subprocess.run([str(probe), str(package / entry['file']), entry['symbol'], fixtures[entry['id']]], check=True)
    print(entry['id'] + ': loaded and parsed')
