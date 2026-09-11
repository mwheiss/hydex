import importlib.util
import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("prepare_surface_runtime.py")
SPEC = importlib.util.spec_from_file_location("prepare_surface_runtime", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def executable(path: Path, body: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o755)
    return path


def codex(path: Path, version: str, hydex: bool = False) -> Path:
    help_text = "--offload --no-offload" if hydex else "codex"
    return executable(
        path,
        "#!/bin/sh\n"
        'case "$1" in\n'
        f"  --version) echo 'codex-cli {version}' ;;\n"
        f"  --help) echo '{help_text}' ;;\n"
        "esac\n",
    )


class SurfaceRuntimeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.original_run_text = MODULE.run_text

        def run_text(command: list[str]) -> str:
            if command[0] == "file":
                return f"{command[1]}: ELF 64-bit x86-64 static-pie linked\n"
            return self.original_run_text(command)

        MODULE.run_text = run_text

    def tearDown(self) -> None:
        MODULE.run_text = self.original_run_text

    def test_desktop_surface_becomes_canonical_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "desktop"
            codex(source / "codex", "0.153.1")
            executable(source / "codex-code-mode-host", "host")
            executable(source / "rg", "rg")
            bwrap = executable(root / "bwrap", "bwrap")
            hydex = codex(root / "hydex", "0.153.1", hydex=True)
            output = root / "runtime"
            summary = MODULE.prepare(
                Namespace(
                    surface="desktop",
                    source_root=source,
                    hydex_bin=hydex,
                    bwrap=bwrap,
                    output_root=output,
                    source_label="chatgpt_26.901.31953_amd64",
                    hydex_commit="a" * 40,
                )
            )

            self.assertEqual(summary["version"], "0.153.1")
            self.assertEqual((output / "bin/codex").read_bytes(), hydex.read_bytes())
            self.assertEqual((output / "bin/codex-code-mode-host").read_text(), "host")
            self.assertEqual((output / "codex-path/rg").read_text(), "rg")
            self.assertEqual((output / "codex-resources/bwrap").read_text(), "bwrap")
            self.assertEqual(
                json.loads((output / "codex-package.json").read_text())["version"],
                "0.153.1",
            )

    def test_mismatched_hydex_is_rejected_before_output(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "desktop"
            codex(source / "codex", "0.153.1")
            executable(source / "codex-code-mode-host", "host")
            executable(source / "rg", "rg")
            bwrap = executable(root / "bwrap", "bwrap")
            hydex = codex(root / "hydex", "0.153.0", hydex=True)
            output = root / "runtime"
            with self.assertRaises(SystemExit):
                MODULE.prepare(
                    Namespace(
                        surface="desktop",
                        source_root=source,
                        hydex_bin=hydex,
                        bwrap=bwrap,
                        output_root=output,
                        source_label="desktop",
                        hydex_commit="b" * 40,
                    )
                )
            self.assertFalse(output.exists())


if __name__ == "__main__":
    unittest.main()
