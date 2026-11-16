"""Regression checks for old/new Mesa ICD names and explicit driver selection."""

import tempfile
import unittest
from pathlib import Path

from vulkan_fixture import lavapipe_icd


class LavapipeFixtureTests(unittest.TestCase):
    def test_old_and_new_package_names(self):
        for name in ("lvp_icd.json", "lvp_icd.x86_64.json"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                driver = root / name
                driver.write_text("{}")
                self.assertEqual(lavapipe_icd(root, {}, "x86_64"), driver)

    def test_a_foreign_architecture_is_not_selected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lvp_icd.i686.json").write_text("{}")
            with self.assertRaisesRegex(RuntimeError, "ICD unavailable"):
                lavapipe_icd(root, {}, "x86_64")

    def test_explicit_driver_is_required_to_exist(self):
        with tempfile.TemporaryDirectory() as directory:
            driver = Path(directory) / "custom.json"
            with self.assertRaisesRegex(RuntimeError, "does not name a file"):
                lavapipe_icd(environment={"LAVAPIPE_ICD": str(driver)})
            driver.write_text("{}")
            self.assertEqual(
                lavapipe_icd(environment={"LAVAPIPE_ICD": str(driver)}), driver
            )


if __name__ == "__main__":
    unittest.main()
