import gzip
import hashlib
import importlib.util
import tarfile
import tempfile
import unittest
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("build_runtime_bundle.py")
SPEC = importlib.util.spec_from_file_location("build_runtime_bundle", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class DeterministicArchiveTests(unittest.TestCase):
    def test_archive_is_reproducible_and_has_normalized_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / "source"
            (source / "bin").mkdir(parents=True)
            executable = source / "bin" / "codex"
            executable.write_bytes(b"codex\n")
            executable.chmod(0o755)
            metadata = source / "manifest.json"
            metadata.write_text('{"schema_version": 1}\n')
            metadata.chmod(0o644)
            first = root / "first.tar.gz"
            second = root / "second.tar.gz"

            MODULE.write_deterministic_archive(source, first, "runtime", 123456789)
            MODULE.write_deterministic_archive(source, second, "runtime", 123456789)

            self.assertEqual(digest(first), digest(second))
            with (
                gzip.open(first, "rb") as stream,
                tarfile.open(fileobj=stream, mode="r:") as archive,
            ):
                members = {member.name: member for member in archive.getmembers()}
            self.assertEqual(
                sorted(members),
                [
                    "runtime",
                    "runtime/bin",
                    "runtime/bin/codex",
                    "runtime/manifest.json",
                ],
            )
            self.assertEqual(members["runtime/bin/codex"].mode, 0o755)
            self.assertEqual(members["runtime/manifest.json"].mode, 0o644)
            self.assertEqual(members["runtime/bin/codex"].mtime, 123456789)
            self.assertEqual(members["runtime/bin/codex"].uid, 0)
            self.assertEqual(members["runtime/bin/codex"].gid, 0)


if __name__ == "__main__":
    unittest.main()
