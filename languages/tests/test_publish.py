import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import publish
from build import TARGETS


class CatalogPublication(unittest.TestCase):
    def test_all_registry_bytes_must_match_before_catalog_exists(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            payloads = {}
            for target in TARGETS:
                folder = directory / target
                folder.mkdir()
                payload = target.encode()
                filename = target + '.tgz'
                url = 'https://registry.npmjs.org/' + filename
                payloads[url] = payload
                (folder / filename).write_bytes(payload)
                (folder / 'catalog-entry.json').write_text(json.dumps({
                    'target': target, 'version': '0.1.0', 'url': url,
                    'size': len(payload), 'sha256': hashlib.sha256(payload).hexdigest(),
                }))
            with patch.object(sys, 'argv', ['publish.py', temporary]), patch.object(
                publish.urllib.request, 'urlopen', side_effect=lambda url, **_: io.BytesIO(payloads[url])
            ):
                publish.main()
            self.assertEqual(len(json.loads((directory / 'catalog.json').read_text())['packages']), 4)
            payloads[next(iter(payloads))] = b'wrong registry bytes'
            with patch.object(sys, 'argv', ['publish.py', temporary]), patch.object(
                publish.urllib.request, 'urlopen', side_effect=lambda url, **_: io.BytesIO(payloads[url])
            ), self.assertRaises(AssertionError):
                publish.main()
            self.assertFalse((directory / 'catalog.json').exists())

    def test_partial_target_set_cannot_produce_a_catalog(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(sys, 'argv', ['publish.py', temporary]), self.assertRaises(AssertionError):
                publish.main()
            self.assertFalse((Path(temporary) / 'catalog.json').exists())


if __name__ == '__main__':
    unittest.main()
