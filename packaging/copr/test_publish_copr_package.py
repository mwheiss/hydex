import importlib.util
import subprocess
import tempfile
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

    def test_package_release_can_advance_without_changing_runtime_release(self) -> None:
        manifest = {
            "schema_version": 1,
            "artifact": {
                "version": "0.153.4",
                "release": 1,
                "target": "x86_64-unknown-linux-musl",
            },
            "provenance": {"release_tag": "hydex-runtime-v0.153.4-r1"},
        }

        rendered = MODULE.render_spec(
            "Release: @RPM_RELEASE@ Source: @ARCHIVE_NAME@ @RPM_VERSION@ @RUNTIME_ROOT@ @RELEASE_TAG@",
            manifest,
            Path("hydex-runtime-0.153.4-r1-x86_64-unknown-linux-musl.tar.gz"),
            package_release=2,
        )

        self.assertIn("Release: 2", rendered)
        self.assertIn("hydex-runtime-v0.153.4-r1", rendered)

    def test_publish_targets_full_matrix_with_network_disabled(self) -> None:
        commands: list[list[str]] = []
        original_run = MODULE.run

        def fake_run(command: list[str], cwd: Path, **_kwargs: object):
            commands.append(command)
            return subprocess.CompletedProcess(
                command,
                0,
                stdout="Created builds: 123\n",
                stderr="",
            )

        MODULE.run = fake_run
        try:
            with tempfile.TemporaryDirectory() as temp:
                build_id = MODULE.publish_srpm(
                    "mheiss",
                    "hydex",
                    Path(temp) / "hydex.src.rpm",
                    Path(temp),
                )
        finally:
            MODULE.run = original_run

        self.assertEqual(build_id, "123")
        self.assertEqual(commands[0][:3], ["copr-cli", "build", "mheiss/hydex"])
        self.assertEqual(commands[0][4:6], ["--enable-net", "off"])
        self.assertEqual(
            [
                commands[0][index + 1]
                for index, value in enumerate(commands[0])
                if value == "--chroot"
            ],
            list(MODULE.CHROOTS),
        )


if __name__ == "__main__":
    unittest.main()
