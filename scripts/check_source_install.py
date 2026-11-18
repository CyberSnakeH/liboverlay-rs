#!/usr/bin/env python3
"""Build external Rust and C++ clients from two trusted local distribution archives."""

import argparse
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path


def unpack(archive, directory):
    # These archives are produced by git archive and cargo package respectively.
    directory.mkdir()
    with tarfile.open(archive, "r:gz") as packed:
        roots = set()
        for member in packed:
            path = Path(member.name)
            if (
                not path.parts
                or path.is_absolute()
                or ".." in path.parts
                or not (member.isfile() or member.isdir())
            ):
                raise ValueError(f"Unexpected archive entry: {member.name}")
            roots.add(path.parts[0])
            if (
                "target" in path.parts
                or ".git" in path.parts
                or path.suffix in (".so", ".a", ".rlib")
                or ".so." in path.name
            ):
                raise ValueError(f"Unexpected build artifact: {member.name}")
        if len(roots) != 1:
            raise ValueError(f"Expected one top-level directory in {archive}")
        # Only regular files and directories beneath one relative root reach here.
        packed.extractall(directory)
    return directory / roots.pop()


def check(source_archive, rust_archive, root, draw, rust_only):
    source = root / "source"
    unpack(source_archive, root / "full-archive").rename(source)
    rust_package = root / "rust-package"
    unpack(rust_archive, root / "crate-archive").rename(rust_package)
    fixtures = source / "tests/consumers"
    shutil.copytree(fixtures / "rust", root / "rust")
    rust_target = root / "cargo-target"
    subprocess.run(
        ["cargo", "run", "--quiet", "--target-dir", rust_target],
        cwd=root / "rust",
        check=True,
    )
    print("Rust consumer passed using the unpacked .crate", flush=True)

    cpp_client = None
    if not rust_only:
        shutil.copytree(fixtures / "cpp", root / "cpp")
        build = root / "build"
        subprocess.run(
            ["cmake", "-S", root / "cpp", "-B", build, "-DCMAKE_BUILD_TYPE=Release"],
            check=True,
        )
        subprocess.run(["cmake", "--build", build], check=True)
        cpp_client = build / "overlay_consumer"
        subprocess.run([cpp_client], check=True)
        print("C++17 consumer passed using the full source archive", flush=True)

    if draw:
        from check_consumer_frames import check_frames

        check_frames(rust_target / "debug/overlay-consumer", cpp_client)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source_archive", type=Path)
    parser.add_argument("rust_archive", type=Path)
    parser.add_argument(
        "--draw",
        action="store_true",
        help="read back client drawings on private displays",
    )
    parser.add_argument(
        "--rust-only",
        action="store_true",
        help="exercise just the external Cargo client",
    )
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="overlay-consumers-") as directory:
        check(
            args.source_archive,
            args.rust_archive,
            Path(directory),
            args.draw,
            args.rust_only,
        )


if __name__ == "__main__":
    main()
