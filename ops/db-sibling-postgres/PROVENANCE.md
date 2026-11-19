# Composition inputs

The source image and sibling root are rebuilt and tested by their existing
hosted experiments at the same checkout before composition. Only the source
image's `/opt/pg-smoke` tree is copied. The resulting file and symlink hashes,
metadata, ownership and resolved dynamic dependencies are checked separately.
This does not establish reproducible builds or image provenance beyond those
recorded experiments.

The source smoke's successful hosted build log shows `psql` linked with
`-lreadline`. The sibling root has `libtinfo6=6.5+20250216-2`. The exact
`libreadline8t64=8.2-6`, `readline-common=8.2-6` and `libtinfo6` pins are in
Debian's [2026-11-18 Trixie snapshot index](https://snapshot.debian.org/archive/debian/20261118T000000Z/dists/trixie/main/binary-amd64/Packages.xz),
whose Release SHA-256 is
`7778d3e3f303b7ddb8ce0fe7c8d57473a076c6bf2e8f241f75421d2396352498`.
`libreadline8t64` depends on `readline-common`, libc and `libtinfo6`. APT
verifies the signed Release using the pinned root's Debian keyring, then the
index and package checksums. Expiration checking is disabled only for the
fixed historical snapshot. Runtime linkage must confirm the actual need.

The final composition layer consumes `zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1`
from a same-run build of `ops/db-zlib-candidate/Dockerfile`, which verifies
Debian security archive signatures and package metadata. The final layer checks
the original version, package architecture and the already reviewed package
SHA-256 `8c27c342230503b73dfde629642c3b352e87b554e06d60cf58772f236a305ceb`
before installing it. The source package file is mounted read-only during the
installation rather than copied into the final image. The signed archive inputs
and package evidence are retained in the composition artifact.
