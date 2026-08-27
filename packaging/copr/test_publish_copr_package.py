import importlib.util
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("publish_copr_package.py")
SPEC = importlib.util.spec_from_file_location("publish_copr_package", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RenderSpecTests(unittest.TestCase):
    def test_renders_versioned_runtime_source(self) -> None:
        template = " ".join(sorted(MODULE.PLACEHOLDERS))
        manifest = {
            "schema_version": 1,
            "artifact": {
                "version": "0.150.0-alpha.8",
                "release": 2,
                "target": "x86_64-unknown-linux-musl",
            },
            "provenance": {"release_tag": "hydex-runtime-v0.150.0-alpha.8-r2"},
        }
        archive = Path(
            "hydex-runtime-0.150.0-alpha.8-r2-x86_64-unknown-linux-musl.tar.gz"
        )

        rendered = MODULE.render_spec(template, manifest, archive)

        self.assertNotIn("@", rendered)
        self.assertIn("0.150.0_alpha.8", rendered)
        self.assertIn("hydex-runtime-v0.150.0-alpha.8-r2", rendered)
        self.assertIn(archive.name, rendered)


if __name__ == "__main__":
    unittest.main()
