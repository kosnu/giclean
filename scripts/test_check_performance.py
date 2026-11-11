import tempfile
from pathlib import Path
import unittest

from check_performance import Repository, regressed


class PerformanceTests(unittest.TestCase):
    def test_only_meaningful_slowdowns_fail(self):
        self.assertFalse(regressed(0.100, 0.129))
        self.assertTrue(regressed(0.100, 0.131))
        self.assertFalse(regressed(0.010, 0.035))  # Big percentage, small absolute change.
        self.assertTrue(regressed(0.010, 0.041))
        self.assertFalse(regressed(1.000, 1.200))  # Big absolute change, small percentage.
        self.assertTrue(regressed(1.000, 1.301))
        self.assertFalse(regressed(1.000, 0.900))

    def test_success_exit_without_deleting_is_rejected(self):
        with tempfile.TemporaryDirectory(prefix="giclean-performance-test-") as directory:
            root = Path(directory)
            # Use the actual fixture and Git; a successful no-op must not pass as fast.
            repo = Repository(root)
            case = next(case for case in repo.cases() if case[0] == "merged20")
            repo.configure(case[1])
            binary = root / "noop"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o700)
            with self.assertRaisesRegex(RuntimeError, "unexpected deletion output"):
                repo.measure(binary, case)


if __name__ == "__main__":
    unittest.main()
