import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("publish_aur_package.py")
SPEC = importlib.util.spec_from_file_location("publish_aur_package", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RenderPkgbuildTests(unittest.TestCase):
    def test_renders_immutable_versioned_source(self) -> None:
        template = " ".join(sorted(MODULE.PLACEHOLDERS))
        manifest = {
            "schema_version": 1,
            "artifact": {"version": "0.150.0-alpha.8", "release": 2},
            "provenance": {"release_tag": "hydex-runtime-v0.150.0-alpha.8-r2"},
        }

        rendered = MODULE.render_pkgbuild(template, manifest, "a" * 64)

        self.assertNotIn("@", rendered)
        self.assertIn("0.150.0.alpha.8", rendered)
        self.assertIn("0.150.0-alpha.8", rendered)
        self.assertIn("hydex-runtime-v0.150.0-alpha.8-r2", rendered)
        self.assertIn("a" * 64, rendered)


if __name__ == "__main__":
    unittest.main()
