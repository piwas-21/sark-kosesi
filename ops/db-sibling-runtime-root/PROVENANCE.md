# Sibling root inputs

The pinned AMD64 manifest is Docker Official Debian `trixie-20261104-slim`,
`sha256:7378260c0a098d27ac206225db5e7d92b8c7c84ea7afccc86eeb513756bbd392`.
Its index is `sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2`
and config `sha256:596c6d80c879dc94eae22cb9c59a5e18f66f4f9722e170892324c282b4eb19bc`.
Its first layer `sha256:37fb86c2a767b88d500c6872195f88078f64765fe0d2f9cc375cc2efbd4c0a41`
is also the first layer of the pinned stock PostgreSQL AMD64 image. The
[official-images mapping](https://github.com/docker-library/official-images/blob/master/library/debian)
points to its [upstream package inventory](https://github.com/debuerreotype/docker-debian-artifacts/blob/373f61a386722a989c79f0c11f23fcf379b56e94/trixie/slim/rootfs.manifest),
whose SHA-256 is `f9ef45f4cfc347030cc056cb27360b408cd3753bf1a8faec58946debeb7f2e5d`.
The hosted check fetches and verifies all identity bytes and the inventory.

The three direct packages are in the [Debian Trixie snapshot index](https://snapshot.debian.org/archive/debian/20261118T000000Z/dists/trixie/main/binary-amd64/Packages.xz).
Its [Release file](https://snapshot.debian.org/archive/debian/20261118T000000Z/dists/trixie/Release)
records index SHA-256 `7778d3e3f303b7ddb8ce0fe7c8d57473a076c6bf2e8f241f75421d2396352498`,
which matched the streamed index during preparation. APT verifies the snapshot
with the pinned root's Debian keyring. Expiration checking is disabled only
for the dated snapshot; package and index signatures remain required. All
installed package changes are checked against the explicit three-package lock.
