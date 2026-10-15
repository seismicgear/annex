import importlib.util
import pathlib
import sys
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('protocol', pathlib.Path(__file__).parents[1] / 'verify-linux-protocol.py')
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)

DESKTOP = '[Desktop Entry]\nType=Application\nTerminal=false\nExec=annex-desktop %u\nMimeType=x-scheme-handler/annex;\n'


class ProtocolTests(unittest.TestCase):
    def test_single_url_passes(self):
        self.assertEqual(protocol.validate_desktop(DESKTOP)['exec'], 'annex-desktop %u')

    def test_actual_broken_package_without_argument_is_rejected(self):
        with self.assertRaises(ValueError):
            protocol.validate_desktop(DESKTOP.replace(' %u', ''))

    def test_quoted_or_multiple_or_wrong_argument_is_rejected(self):
        for value in ['"%u"', '%U', '%f', '%u %u', '%u --other']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                protocol.validate_desktop(DESKTOP.replace('%u', value))

    def test_missing_mime_is_rejected(self):
        with self.assertRaises(ValueError):
            protocol.validate_desktop(DESKTOP.replace('x-scheme-handler/annex', 'text/plain'))

    def test_cache_provider_must_be_required(self):
        for value in ['libgtk-3-0', 'desktop-file-utils | other', 'other-desktop-file-utils']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                protocol.validate_dependencies(value)
        protocol.validate_dependencies('libgtk-3-0, desktop-file-utils (>= 0.26)')


if __name__ == '__main__':
    unittest.main()
