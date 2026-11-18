# Build dependency provenance

The direct package versions in `packages.lock` are from the Debian Trixie
AMD64 index at the [2026-11-18 snapshot](https://snapshot.debian.org/archive/debian/20261118T000000Z/dists/trixie/main/binary-amd64/Packages.xz).
The corresponding [Release](https://snapshot.debian.org/archive/debian/20261118T000000Z/dists/trixie/Release)
identifies `trixie`, is dated 2026-09-12 07:55:41 UTC and specifies the index
SHA-256 `7778d3e3f303b7ddb8ce0fe7c8d57473a076c6bf2e8f241f75421d2396352498`.
The streamed index matched that hash when the lock was prepared. The hosted
build independently uses APT verification with the Debian archive keyring from
the pinned base, and checks that each installed direct package has its locked
version. The snapshot's expiry check is disabled because its date is fixed;
APT signature, index and package verification remain enabled. Transitive
packages are selected from this snapshot, not individually locked. Package
versions, including transitive packages, are retained in build evidence.

The snapshot is intentionally historical for this disposable experiment. It is
not a vulnerability disposition or a production package-update policy.
