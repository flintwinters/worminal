"""The Python test runner's flaky-failure handling."""
import unittest

from tools.test_runner import failed_tests

OUTPUT = """
======================================================================
FAIL: test_alternate_scroll (test_keys.NavigationKeysTest.test_alternate_scroll)
----------------------------------------------------------------------
AssertionError: window

======================================================================
FAIL: test_zoom (test_native.NativeTerminalTest.test_zoom) (style=2)
----------------------------------------------------------------------
======================================================================
ERROR: test_alternate_scroll (test_keys.NavigationKeysTest.test_alternate_scroll)
----------------------------------------------------------------------
FAIL: test_text (test_native.NativeTerminalTest.test_text) mentions FAIL: not_a_test (x.y)
"""


class FailedTestsTest(unittest.TestCase):
    def test_ids_come_from_failure_headers_once_each_in_order(self):
        self.assertEqual(failed_tests(OUTPUT), [
            'test_keys.NavigationKeysTest.test_alternate_scroll',
            'test_native.NativeTerminalTest.test_zoom',
            'test_native.NativeTerminalTest.test_text'])

    def test_clean_output_names_nothing(self):
        self.assertEqual(failed_tests('Ran 3 tests in 0.1s\n\nOK\n'), [])


if __name__ == '__main__':
    unittest.main()
