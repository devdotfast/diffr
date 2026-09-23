"""Build release archives and a Homebrew formula from their exact bytes."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import tarfile

TARGETS = ("aarch64-apple-darwin", "x86_64-unknown-linux-gnu")
ROOT = Path(__file__).resolve().parent.parent


def archive_name(version, target):
    return f"diffr-{version}-{target}.tar.gz"


def pack(version, target, install, output):
    actual = subprocess.check_output([install / "bin/diffr", "--version"], text=True).strip()
    if actual != f"diffr {version}":
        raise ValueError(f"Release version mismatch: {actual}")
    with tarfile.open(output / archive_name(version, target), "w:gz") as archive:
        for name in ("diffr", "diffr-tui"):
            archive.add(install / "bin" / name, arcname=name)
        for name in ("LICENSE", "NOTICE", "tui/LICENSE", "tui/themes/LICENSE"):
            archive.add(ROOT / name, arcname=name)


def formula(version, output):
    checksums = {}
    for target in TARGETS:
        name = archive_name(version, target)
        checksums[target] = hashlib.sha256((output / name).read_bytes()).hexdigest()
    (output / "SHA256SUMS").write_text("".join(
        f"{checksums[target]}  {archive_name(version, target)}\n" for target in TARGETS
    ))
    url = f"https://github.com/devdotfast/diffr/releases/download/{version}/diffr-{version}"
    (output / "diffr.rb").write_text(f'''class Diffr < Formula
  desc "Structural diffs with an interactive terminal frontend"
  homepage "https://github.com/devdotfast/diffr"
  version "{version}"
  license all_of: ["MIT", "MPL-2.0"]

  on_macos do
    depends_on arch: :arm64
    url "{url}-aarch64-apple-darwin.tar.gz"
    sha256 "{checksums[TARGETS[0]]}"
  end

  on_linux do
    depends_on arch: :x86_64
    url "{url}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "{checksums[TARGETS[1]]}"
  end

  def install
    bin.install "diffr", "diffr-tui"
    doc.install "NOTICE"
    (pkgshare/"licenses").install "LICENSE"
    (pkgshare/"licenses/tui").install "tui/LICENSE"
    (pkgshare/"licenses/themes").install "tui/themes/LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/diffr --version")
    assert_match "theme", shell_output("#{{bin}}/diffr config schema")
    assert_predicate bin/"diffr-tui", :executable?
  end
end
''')


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("pack", "formula"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--install", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.version):
        parser.error("version must be a stable X.Y.Z release")
    args.output.mkdir(parents=True, exist_ok=True)
    if args.command == "pack":
        if not args.target or not args.install:
            parser.error("pack requires --target and --install")
        pack(args.version, args.target, args.install.resolve(), args.output)
    else:
        formula(args.version, args.output)
