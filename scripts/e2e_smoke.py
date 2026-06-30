#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

import subprocess
import sys
from collections.abc import Callable, Sequence
from pathlib import Path


CommandRunner = Callable[[list[str], Path], int]

IMAGE_TAG = "brain3-mcp-vault-tools:e2e-local"


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def docker_build_command() -> list[str]:
    return [
        "docker",
        "build",
        "-f",
        "./brain3-mcp-vault-tools/Containerfile",
        "-t",
        IMAGE_TAG,
        "./brain3-mcp-vault-tools",
    ]


def cargo_test_command(extra_args: Sequence[str]) -> list[str]:
    return [
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
        *extra_args,
    ]


def subprocess_runner(command: list[str], cwd: Path) -> int:
    try:
        subprocess.run(command, cwd=cwd, check=True)
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


def run(
    extra_args: Sequence[str],
    run_command: CommandRunner = subprocess_runner,
) -> int:
    root = repo_root()
    build_exit_code = run_command(docker_build_command(), root)
    if build_exit_code != 0:
        print(
            f"Docker image build failed with exit code {build_exit_code}; "
            "aborting before running the E2E smoke test.",
            file=sys.stderr,
        )
        return build_exit_code

    return run_command(cargo_test_command(extra_args), root)


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
