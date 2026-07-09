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

VAULT_TOOLS_IMAGE_TAG = "brain3-mcp-vault-tools:e2e-local"
HELLO_MCP_IMAGE_TAG = "brain3-e2e-hello-mcp:e2e-local"
DEFAULT_E2E_TESTS = [
    "e2e_smoke_1_local_docker",
    "e2e_smoke_2_oauth_public_flow",
    "e2e_smoke_3_oauth_quick_tunnel",
    "e2e_smoke_5_plugin_mcp_container",
]


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def docker_build_commands() -> list[list[str]]:
    return [
        [
            "docker",
            "build",
            "-f",
            "./brain3-mcp-vault-tools/Containerfile",
            "-t",
            VAULT_TOOLS_IMAGE_TAG,
            "./brain3-mcp-vault-tools",
        ],
        [
            "docker",
            "build",
            "-f",
            "./testdata/e2e_hello_mcp_container/Containerfile",
            "-t",
            HELLO_MCP_IMAGE_TAG,
            "./testdata/e2e_hello_mcp_container",
        ],
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
        "--test-threads=1",
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
    for build_command in docker_build_commands():
        build_exit_code = run_command(build_command, root)
        if build_exit_code != 0:
            print(
                f"Docker image build failed with exit code {build_exit_code}; "
                "aborting before running the E2E smoke test.",
                file=sys.stderr,
            )
            return build_exit_code

    if extra_args:
        return run_command(cargo_test_command(extra_args), root)

    for test_name in DEFAULT_E2E_TESTS:
        test_exit_code = run_command(cargo_test_command([test_name]), root)
        if test_exit_code != 0:
            return test_exit_code

    return 0


def main() -> int:
    return run(sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
