import importlib.util
import itertools
import json
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("select_surface_runtime.py")
SPEC = importlib.util.spec_from_file_location("select_surface_runtime", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def write_runtime(root: Path, version: str) -> Path:
    runtime = root / version
    (runtime / "bin").mkdir(parents=True)
    (runtime / "codex-package.json").write_text(
        json.dumps(
            {
                "version": version,
                "target": "x86_64-unknown-linux-musl",
                "variant": "codex",
                "entrypoint": "bin/codex",
            }
        )
    )
    binary = runtime / "bin" / "codex"
    binary.write_text(
        "#!/bin/sh\n"
        'case "$1" in\n'
        f"  --version) echo 'codex-cli {version}' ;;\n"
        "  --help) echo '--offload --no-offload' ;;\n"
        "esac\n"
    )
    binary.chmod(0o755)
    return runtime


class VersionTests(unittest.TestCase):
    def test_semver_ordering(self) -> None:
        ordered = [
            "0.151.0-alpha.7.1",
            "0.151.0-alpha.7.2",
            "0.151.0",
            "0.153.0",
            "0.153.1",
        ]
        for left, right in itertools.pairwise(ordered):
            self.assertLess(MODULE.compare_versions(left, right), 0)
            self.assertGreater(MODULE.compare_versions(right, left), 0)
        self.assertEqual(MODULE.compare_versions("0.153.1", "0.153.1"), 0)

    def test_newer_desktop_runtime_is_selected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plugin = write_runtime(root / "plugin", "0.153.0")
            desktop = write_runtime(root / "desktop", "0.153.1")
            result = MODULE.matrix(plugin, desktop)
            self.assertEqual(result["selected"]["surface"], "desktop")
            self.assertEqual(result["selected"]["version"], "0.153.1")
            self.assertTrue(result["versionsDiverge"])

    def test_plugin_runtime_wins_ties(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            plugin = write_runtime(root / "plugin", "0.153.1")
            desktop = write_runtime(root / "desktop", "0.153.1")
            result = MODULE.matrix(plugin, desktop)
            self.assertEqual(result["selected"]["surface"], "plugin")
            self.assertFalse(result["versionsDiverge"])

    def test_metadata_binary_mismatch_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            runtime = write_runtime(Path(temp), "0.153.1")
            (runtime / "codex-package.json").write_text(
                json.dumps({"version": "0.153.0"})
            )
            with self.assertRaises(SystemExit):
                MODULE.read_runtime("desktop", runtime)


if __name__ == "__main__":
    unittest.main()
