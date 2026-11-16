"""Low-level process/exit-status checks for the shared acceptance harness."""
import unittest
from unittest.mock import Mock, patch

from runtime import ProductionSession, RuntimeErrorWithLog


class ProcessExitTests(unittest.TestCase):
    def test_clean_and_abnormal_exits(self):
        for code in (0, 1, -6, -11):
            with self.subTest(code=code), patch("runtime.port_is_open", return_value=False):
                session = ProductionSession(4198)
                process = Mock(returncode=code)
                process.poll.return_value = code
                session.process = process
                if code == 0:
                    session.close()
                else:
                    with self.assertRaisesRegex(RuntimeErrorWithLog, f"exited with code {code}"):
                        session.close()
                self.assertIsNone(session.process)
                process.kill.assert_not_called()


if __name__ == "__main__":
    unittest.main()
