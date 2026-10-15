import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('observe_host', Path(__file__).with_name('observe-host.py'))
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)


class ProcessIdentityTests(unittest.TestCase):
    def test_only_the_exact_installed_executable_is_selected(self):
        rows = ('  12 /Applications/Annex.app/Contents/MacOS/annex-desktop\n'
                '  13 /tmp/annex-desktop\n'
                '  14 /Applications/Annex.app/Contents/MacOS/annex-desktop-other\n')
        self.assertEqual(observer.app_processes(rows), [12])

    def test_actual_open_database_path_is_deduplicated_without_guessing_home(self):
        path = '/private/var/folders/test/Library/Application Support/Annex/annex.db'
        rows = f'p12\nn{path}\nn{path}-wal\nn{path}\nn/tmp/not-annex.db\n'
        self.assertEqual(observer.open_database_paths(rows), [path])

    def test_ambiguous_databases_are_not_silently_chosen(self):
        self.assertEqual(len(observer.open_database_paths('n/a/annex.db\nn/b/annex.db\n')), 2)


if __name__ == '__main__':
    unittest.main()
