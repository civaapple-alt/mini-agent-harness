import hashlib
import importlib.util
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("package_release.py")
SPEC = importlib.util.spec_from_file_location("package_release", SCRIPT)
assert SPEC and SPEC.loader
package_release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(package_release)


class PackageReleaseTests(unittest.TestCase):
    def test_tar_archives_are_deterministic(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "mini-codex"
            binary.write_bytes(b"release-binary")
            first = root / "first"
            second = root / "second"

            first_archive, _ = package_release.package_release(
                binary, "x86_64-unknown-linux-gnu", "1.2.3", first
            )
            second_archive, _ = package_release.package_release(
                binary, "x86_64-unknown-linux-gnu", "1.2.3", second
            )

            self.assertEqual(sha256(first_archive), sha256(second_archive))

    def test_rejects_non_semver_versions(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / "mini-codex.exe"
            binary.write_bytes(b"release-binary")

            with self.assertRaisesRegex(ValueError, "strict SemVer"):
                package_release.package_release(
                    binary,
                    "x86_64-pc-windows-msvc",
                    "v1.2",
                    pathlib.Path(directory) / "out",
                )


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


if __name__ == "__main__":
    unittest.main()
