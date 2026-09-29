import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from definitions import ROOT, definitions, validate


class DefinitionChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'], cwd=ROOT))
        cls.items = definitions()

    def test_current_definitions_and_shared_ocaml_feature(self):
        validate(self.items, self.metadata)
        ocaml = [d for d in self.items if d['feature'] == 'lang-ocaml']
        self.assertEqual({d['id'] for d in ocaml}, {'ocaml', 'ocaml-interface'})

    def test_missing_registry_and_feature_entries_are_rejected(self):
        with self.assertRaisesRegex(AssertionError, 'registry'):
            validate(self.items[1:], self.metadata)
        metadata = copy.deepcopy(self.metadata)
        package = next(p for p in metadata['packages'] if p['name'] == 'diffr-cli')
        package['features']['all-languages'].remove(self.items[0]['feature'])
        with self.assertRaisesRegex(AssertionError, 'all-languages'):
            validate(self.items, metadata)

    def test_missing_fixtures_are_rejected(self):
        items = copy.deepcopy(self.items)
        items[0]['fixture']['prefix'] = 'missing-extra-language-fixture'
        with self.assertRaisesRegex(AssertionError, 'missing fixture'):
            validate(items, self.metadata)


if __name__ == '__main__':
    unittest.main()
