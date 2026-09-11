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

    def test_package_release_can_advance_without_changing_runtime_release(self) -> None:
        manifest = {
            "schema_version": 1,
            "artifact": {"version": "0.153.4", "release": 1},
            "provenance": {"release_tag": "hydex-runtime-v0.153.4-r1"},
        }

        rendered = MODULE.render_pkgbuild(
            "pkgrel=@PKGREL@ runtime=@RUNTIME_RELEASE@ version=@PKGVER@ tag=@RELEASE_TAG@ hash=@ARCHIVE_SHA256@",
            manifest,
            "a" * 64,
            package_release=2,
        )

        self.assertIn("pkgrel=2 runtime=1", rendered)


if __name__ == "__main__":
    unittest.main()
