"""Build one Rust test fixture and retain normal compiler diagnostics."""

import json
from pathlib import Path
import subprocess
import sys


def build_test(name, features):
    command = [
        "cargo",
        "test",
        "--locked",
        "--no-default-features",
        "--features",
        features,
        "--test",
        name,
        "--no-run",
        "--message-format=json",
    ]
    result = subprocess.run(
        command,
        cwd=Path(__file__).resolve().parents[1],
        text=True,
        stdout=subprocess.PIPE,
    )
    executable = None
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-message":
            print(message["message"].get("rendered", ""), end="", file=sys.stderr)
        if message.get("target", {}).get("name") == name and message.get("executable"):
            executable = message["executable"]
    if result.returncode:
        raise subprocess.CalledProcessError(result.returncode, command)
    if not executable:
        raise RuntimeError(f"Cargo did not produce {name}")
    return executable
