"""Locate the native lavapipe ICD across Mesa packaging conventions."""

import os
import platform
from pathlib import Path


def lavapipe_icd(
    directory=Path("/usr/share/vulkan/icd.d"), environment=None, machine=None
):
    environment = os.environ if environment is None else environment
    override = environment.get("LAVAPIPE_ICD")
    if override:
        driver = Path(override)
        if not driver.is_file():
            raise RuntimeError(f"LAVAPIPE_ICD does not name a file: {driver}")
        return driver
    architecture = platform.machine() if machine is None else machine
    # Mesa 25 packaging may omit the architecture suffix; older packages retain it.
    for name in ("lvp_icd.json", f"lvp_icd.{architecture}.json"):
        candidate = directory / name
        if candidate.is_file():
            return candidate
    raise RuntimeError(
        "lavapipe ICD unavailable; install Mesa Vulkan drivers or set LAVAPIPE_ICD"
    )


if __name__ == "__main__":
    print(lavapipe_icd())
