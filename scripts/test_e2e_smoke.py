import importlib.util
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("e2e_smoke.py")


def load_script():
    spec = importlib.util.spec_from_file_location("e2e_smoke", SCRIPT_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class E2ESmokeScriptTests(unittest.TestCase):
    def test_runs_build_before_cargo_and_forwards_extra_args(self):
        module = load_script()
        calls = []

        def fake_run(command, cwd):
            calls.append((command, cwd))
            return 0

        exit_code = module.run(["e2e_smoke_starts_gateway"], run_command=fake_run)

        self.assertEqual(exit_code, 0)
        self.assertEqual(len(calls), 2)
        self.assertEqual(
            calls[0][0],
            [
                "docker",
                "build",
                "-f",
                "./brain3-mcp-vault-tools/Containerfile",
                "-t",
                "brain3-mcp-vault-tools:e2e-local",
                "./brain3-mcp-vault-tools",
            ],
        )
        self.assertEqual(
            calls[1][0],
            [
                "cargo",
                "test",
                "-p",
                "brain3",
                "--test",
                "e2e_smoke",
                "--features",
                "e2e",
                "--",
                "--nocapture",
                "e2e_smoke_starts_gateway",
            ],
        )

    def test_build_failure_aborts_before_cargo_runs(self):
        module = load_script()
        calls = []

        def fake_run(command, cwd):
            calls.append((command, cwd))
            return 42

        exit_code = module.run([], run_command=fake_run)

        self.assertEqual(exit_code, 42)
        self.assertEqual(len(calls), 1)
        self.assertEqual(calls[0][0][0:2], ["docker", "build"])


if __name__ == "__main__":
    unittest.main()
