import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from licenses import collect


class LicenseChecks(unittest.TestCase):
    def test_locked_download_cache_and_integrity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            revision = 'a' * 40
            (root / '.cargo_vcs_info.json').write_text(json.dumps({'git': {'sha1': revision}}))
            crate = dict(name='test', manifest_path=str(root / 'Cargo.toml'), repository='https://github.com/example/grammar')
            data = b'Upstream license\n'
            spec = dict(path='LICENSE', sha256=hashlib.sha256(data).hexdigest())
            cache = root / 'cache'
            with patch('licenses.urlopen', return_value=io.BytesIO(data)) as fetch:
                self.assertEqual(collect(crate, spec, cache), data)
                fetch.assert_called_once_with(f'https://raw.githubusercontent.com/example/grammar/{revision}/LICENSE', timeout=30)
            with patch('licenses.urlopen', side_effect=AssertionError('unexpected network')):
                self.assertEqual(collect(crate, spec, cache), data)
                (cache / spec['sha256']).write_bytes(b'corrupt')
                with self.assertRaisesRegex(AssertionError, 'hash mismatch'):
                    collect(crate, spec, cache)
                (root / 'LICENSE').write_bytes(data)
                self.assertEqual(collect(crate, spec, cache), data)
            with self.assertRaisesRegex(AssertionError, 'invalid license path'):
                collect(crate, dict(spec, path='../LICENSE'), cache)

    def test_failed_download_does_not_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / '.cargo_vcs_info.json').write_text(json.dumps({'git': {'sha1': 'b' * 40}}))
            crate = dict(name='test', manifest_path=str(root / 'Cargo.toml'), repository='https://github.com/example/grammar')
            with patch('licenses.urlopen', return_value=io.BytesIO(b'wrong license')):
                with self.assertRaisesRegex(AssertionError, 'hash mismatch'):
                    collect(crate, dict(path='LICENSE', sha256='0' * 64), root / 'cache')
            self.assertFalse((root / 'cache').exists())


if __name__ == '__main__':
    unittest.main()
