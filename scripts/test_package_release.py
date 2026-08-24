import hashlib
import importlib.util
import pathlib
import tarfile
import tempfile
import unittest
import zipfile


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

    def test_tar_archive_contents_modes_and_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "mini-codex"
            binary.write_bytes(b"linux-release-binary")

            archive, checksum = package_release.package_release(
                binary, "x86_64-unknown-linux-gnu", "1.2.3", root / "out"
            )

            package_name = "mini-codex-v1.2.3-x86_64-unknown-linux-gnu"
            with tarfile.open(archive, "r:gz") as contents:
                names = contents.getnames()
                executable = contents.getmember(f"{package_name}/mini-codex")
                readme = contents.getmember(f"{package_name}/README.md")
                payload = contents.extractfile(executable)

                self.assertEqual(
                    names,
                    [
                        package_name,
                        f"{package_name}/mini-codex",
                        f"{package_name}/README.md",
                        f"{package_name}/LICENSE",
                        f"{package_name}/CHANGELOG.md",
                    ],
                )
                self.assertEqual(executable.mode, 0o755)
                self.assertEqual(readme.mode, 0o644)
                self.assertIsNotNone(payload)
                self.assertEqual(payload.read(), b"linux-release-binary")
            self.assert_checksum(archive, checksum)

    def test_zip_archive_is_deterministic_and_has_expected_contents(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "mini-codex.exe"
            binary.write_bytes(b"windows-release-binary")
            first_archive, checksum = package_release.package_release(
                binary, "x86_64-pc-windows-msvc", "1.2.3", root / "first"
            )
            second_archive, _ = package_release.package_release(
                binary, "x86_64-pc-windows-msvc", "1.2.3", root / "second"
            )

            package_name = "mini-codex-v1.2.3-x86_64-pc-windows-msvc"
            with zipfile.ZipFile(first_archive) as contents:
                names = contents.namelist()
                executable = contents.getinfo(f"{package_name}/mini-codex.exe")

                self.assertEqual(
                    names,
                    [
                        f"{package_name}/mini-codex.exe",
                        f"{package_name}/README.md",
                        f"{package_name}/LICENSE",
                        f"{package_name}/CHANGELOG.md",
                    ],
                )
                self.assertEqual(executable.external_attr >> 16, 0o755)
                self.assertEqual(
                    contents.read(executable), b"windows-release-binary"
                )
            self.assertEqual(sha256(first_archive), sha256(second_archive))
            self.assert_checksum(first_archive, checksum)

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

    def assert_checksum(
        self, archive: pathlib.Path, checksum: pathlib.Path
    ) -> None:
        self.assertEqual(
            checksum.read_text(encoding="ascii"),
            f"{sha256(archive)}  {archive.name}\n",
        )


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


if __name__ == "__main__":
    unittest.main()
