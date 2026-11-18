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
