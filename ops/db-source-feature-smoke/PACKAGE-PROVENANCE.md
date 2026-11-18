# Build dependency provenance

Most direct package versions in `packages.lock` are from the Debian Trixie
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

The OpenSSL development and shared-library pins instead come from the
[Debian security snapshot](https://snapshot.debian.org/archive/debian-security/20261118T000000Z/dists/trixie-security/main/binary-amd64/Packages.xz).
Its [Release](https://snapshot.debian.org/archive/debian-security/20261118T000000Z/dists/trixie-security/Release)
is dated 2026-11-17 22:33:40 UTC and records the AMD64 index SHA-256
`334e342e487a7dc95eef769d7dce4843053e686eef92e11416243149fc0366ea`;
the streamed index matched it. The index gives both packages version
`3.5.7-1~deb13u3` and makes the development package depend on exactly that
shared-library version. The builder first verifies that the immutable stock
base has the same shared-library version. APT verifies both archive signatures
with the same scoped Debian keyring. The final image retains the stock runtime.

The matching `curl`/`libcurl4t64` and `zlib1g-dev`/`zlib1g` pairs are
explicitly pinned to the main snapshot versions so adding the security index
does not make APT select incompatible newer companion packages.
