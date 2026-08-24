#!/usr/bin/env python3

import argparse
import gzip
import hashlib
import io
import pathlib
import re
import tarfile
import zipfile


ROOT = pathlib.Path(__file__).resolve().parents[1]
PUBLIC_FILES = ("README.md", "LICENSE", "CHANGELOG.md")
SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
ZIP_TIME = (1980, 1, 1, 0, 0, 0)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Package one mini-codex release binary")
    parser.add_argument("--binary", required=True, type=pathlib.Path)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", default=pathlib.Path("dist"), type=pathlib.Path)
    return parser.parse_args()


def package_release(
    binary: pathlib.Path,
    target: str,
    version: str,
    output: pathlib.Path,
) -> tuple[pathlib.Path, pathlib.Path]:
    if not SEMVER.fullmatch(version):
        raise ValueError(f"version is not strict SemVer: {version}")
    if not binary.is_file():
        raise ValueError(f"release binary does not exist: {binary}")
    for name in PUBLIC_FILES:
        if not (ROOT / name).is_file():
            raise ValueError(f"release input does not exist: {ROOT / name}")

    output.mkdir(parents=True, exist_ok=True)
    package_name = f"mini-codex-v{version}-{target}"
    windows = "windows" in target
    archive = output / f"{package_name}{'.zip' if windows else '.tar.gz'}"
    checksum = archive.with_name(f"{archive.name}.sha256")
    if archive.exists() or checksum.exists():
        raise ValueError(f"release output already exists: {archive}")

    members = [(binary, "mini-codex.exe" if windows else "mini-codex", 0o755)]
    members.extend((ROOT / name, name, 0o644) for name in PUBLIC_FILES)
    if windows:
        write_zip(archive, package_name, members)
    else:
        write_tar_gz(archive, package_name, members)

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    checksum.write_text(f"{digest}  {archive.name}\n", encoding="ascii", newline="\n")
    return archive, checksum


def write_tar_gz(
    archive: pathlib.Path,
    package_name: str,
    members: list[tuple[pathlib.Path, str, int]],
) -> None:
    with archive.open("xb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as output:
                directory = tarfile.TarInfo(package_name)
                normalize_tar_info(directory, 0o755)
                directory.type = tarfile.DIRTYPE
                output.addfile(directory)
                for source, name, mode in members:
                    payload = source.read_bytes()
                    info = tarfile.TarInfo(f"{package_name}/{name}")
                    normalize_tar_info(info, mode)
                    info.size = len(payload)
                    output.addfile(info, io.BytesIO(payload))


def normalize_tar_info(info: tarfile.TarInfo, mode: int) -> None:
    info.mode = mode
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"


def write_zip(
    archive: pathlib.Path,
    package_name: str,
    members: list[tuple[pathlib.Path, str, int]],
) -> None:
    with zipfile.ZipFile(archive, mode="x", compression=zipfile.ZIP_DEFLATED) as output:
        for source, name, mode in members:
            info = zipfile.ZipInfo(f"{package_name}/{name}", date_time=ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = mode << 16
            output.writestr(info, source.read_bytes())


def main() -> int:
    arguments = parse_args()
    try:
        archive, checksum = package_release(
            arguments.binary,
            arguments.target,
            arguments.version,
            arguments.output,
        )
    except (OSError, ValueError) as error:
        print(f"error: {error}")
        return 1
    print(archive)
    print(checksum)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
