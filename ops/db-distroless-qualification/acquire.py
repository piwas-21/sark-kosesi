"""Acquire bounded public, digest-pinned assembly inputs on the hosted runner."""

import argparse
import os
import subprocess
import urllib.parse
import urllib.request
from pathlib import Path, PurePosixPath

from assembly import read_json, scratch_bytes

from packages import HERE, digest, owned_directory, require

PUBLIC_HOSTS = {
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
    "snapshot.debian.org",
    "snapshot-cloudflare.debian.org",
}


def public_host(url):
    parsed = urllib.parse.urlsplit(url)
    return parsed.scheme == "https" and parsed.hostname in PUBLIC_HOSTS and not parsed.username


class PublicRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        require(public_host(newurl), "Unexpected public input redirect")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def fetch(url, destination, limit, *, expected=None):
    require(public_host(url), "Unexpected public input origin")
    opener = urllib.request.build_opener(PublicRedirect())
    request = urllib.request.Request(url, headers={"Accept-Encoding": "identity"})
    size = 0
    with opener.open(request, timeout=30) as response, destination.open("xb") as output:
        require(response.status == 200, "Unexpected public input response")
        while block := response.read(65536):
            size += len(block)
            require(size <= limit, "Public input exceeds byte limit")
            output.write(block)
    require(size > 0, "Empty public input")
    if expected is not None:
        require(digest(destination) == expected, "Public input digest differs")
        require(size == limit, "Exact public input size differs")


def install_tools(folder, locked):
    cosign = folder / "cosign"
    regctl = folder / "regctl-linux-amd64"
    bundle = folder / "regctl.bundle.json"
    version = locked["cosign"]["version"]
    release = f"https://github.com/sigstore/cosign/releases/download/{version}"
    fetch(release + "/cosign-linux-amd64", cosign, 145753267, expected=locked["cosign"]["sha256"])
    cosign.chmod(0o700)
    version = locked["regctl"]["version"]
    release = f"https://github.com/regclient/regclient/releases/download/{version}"
    fetch(release + "/regctl-linux-amd64", regctl, 12947580, expected=locked["regctl"]["sha256"])
    fetch(
        release + "/regctl-linux-amd64.sigstore.json",
        bundle,
        10115,
        expected="75fbdfcd83ae29cc17a8a2b352e2cc5b7759c5998691bfbcd5dfb1483f77094c",
    )
    subprocess.run(
        [
            str(cosign),
            "verify-blob",
            "--bundle",
            str(bundle),
            "--certificate-oidc-issuer",
            "https://token.actions.githubusercontent.com",
            "--certificate-identity",
            f"https://github.com/regclient/regclient/.github/workflows/go.yml@refs/tags/{version}",
            str(regctl),
        ],
        check=True,
        timeout=120,
    )
    regctl.chmod(0o700)
    return cosign, regctl


def acquire(folder):
    owned_directory(folder)
    apt = owned_directory(folder / "debian-inputs")
    require(not os.path.lexists(folder / "images"), "Unexpected existing OCI layout path")
    locks = read_json(HERE / "assembly-inputs.json")
    require(digest(HERE / "inputs.json") == locks["inputs_sha256"], "Input lock differs")
    inputs = read_json(HERE / "inputs.json")
    cosign, regctl = install_tools(folder, locks)
    base, debian = inputs["base"], inputs["debian"]
    for identity in (base["index"], base["amd64_manifest"]):
        subprocess.run(
            [
                str(cosign),
                "verify",
                "--certificate-oidc-issuer",
                base["oidc_issuer"],
                "--certificate-identity",
                base["signer"],
                f"{base['reference']}@{identity}",
            ],
            check=True,
            timeout=120,
        )
    with (folder / "vendor-index.json").open("xb") as output:
        subprocess.run(
            [
                str(regctl),
                "manifest",
                "get",
                "--format",
                "raw-body",
                f"{base['reference']}@{base['index']}",
            ],
            stdout=output,
            check=True,
            timeout=90,
        )
    subprocess.run(
        [
            str(regctl),
            "image",
            "copy",
            f"{base['reference']}@{base['amd64_manifest']}",
            f"ocidir://{folder}/images:vendor",
        ],
        check=True,
        timeout=180,
    )
    cosign.unlink()
    bundle = folder / "regctl.bundle.json"
    bundle.unlink()
    scratch_bytes(folder)
    origin = debian["origin"]
    release = origin + "dists/trixie/"
    fetch(release + "Release", apt / "Release", 2 * 1024**2)
    require(digest(apt / "Release") == debian["release_sha256"], "Pinned Debian Release differs")
    fetch(release + "Release.gpg", apt / "Release.gpg", 128 * 1024)
    fetch(
        release + debian["index_path"],
        apt / "Packages.xz",
        debian["index_bytes"],
        expected=debian["index_sha256"],
    )
    # The archive keyring has already been extracted from the verified stock image.
    require(
        digest(apt / "debian-archive-keyring.gpg") == debian["keyring_sha256"],
        "Pinned Debian archive keyring differs",
    )
    records = debian["packages"] | locks["tools_packages"]
    require(len(records) == 20, "Unexpected signed package closure size")
    for name, record in sorted(records.items()):
        relative = PurePosixPath(record["filename"])
        require(
            relative.parts[0] == "pool"
            and ".." not in relative.parts
            and relative.suffix == ".deb",
            "Unexpected package archive path",
        )
        fetch(
            origin + str(relative),
            apt / (name + ".deb"),
            record["bytes"],
            expected=record["sha256"],
        )
    scratch_bytes(folder)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    args = parser.parse_args()
    acquire(args.folder)
