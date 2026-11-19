# Disposable feature experiment. The retained stock root filesystem is not a release candidate.
FROM mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452 AS build
COPY packages.lock /tmp/smoke-packages.lock
RUN --mount=type=bind,source=ca-certificates.crt,target=/tmp/runner-ca.crt \
    set -eu; \
    test "$(dpkg-query -W -f='${Version}' libssl3t64)" = '3.5.7-1~deb13u3'; \
    printf '%s\n' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg check-valid-until=no] https://snapshot.debian.org/archive/debian/20261118T000000Z/ trixie main' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg check-valid-until=no] https://snapshot.debian.org/archive/debian-security/20261118T000000Z/ trixie-security main' > /tmp/smoke-sources.list; \
    rm -rf /var/lib/apt/lists/*; \
    apt-get -o Acquire::https::CAInfo=/tmp/runner-ca.crt -o Dir::Etc::sourcelist=/tmp/smoke-sources.list -o Dir::Etc::sourceparts=- -o Acquire::AllowInsecureRepositories=false -o Acquire::AllowDowngradeToInsecureRepositories=false -o APT::Get::AllowUnauthenticated=false -o APT::Update::Error-Mode=any update; \
    xargs -r apt-get -y --no-install-recommends --allow-downgrades -o Acquire::https::CAInfo=/tmp/runner-ca.crt -o Dir::Etc::sourcelist=/tmp/smoke-sources.list -o Dir::Etc::sourceparts=- -o APT::Get::AllowUnauthenticated=false install < /tmp/smoke-packages.lock; \
    while IFS='=' read -r package version; do test "$(dpkg-query -W -f='${Version}' "$package")" = "$version"; done < /tmp/smoke-packages.lock; \
    mkdir -p /build /smoke-build-evidence; \
    dpkg-query -W -f='${binary:Package}=${Version}\n' | LC_ALL=C sort > /smoke-build-evidence/build-packages.txt; \
    sha256sum /usr/share/keyrings/debian-archive-keyring.gpg /tmp/runner-ca.crt /tmp/smoke-packages.lock > /smoke-build-evidence/trust-inputs.txt; \
    curl --fail --location --proto '=https' --tlsv1.2 --output /build/postgresql.tar.bz2 https://ftp.postgresql.org/pub/source/v18.7/postgresql-18.7.tar.bz2; \
    echo '441dfae4280cae3e0425e5f876f80559fb0731c140f1aae22b3e5a54d0411bfd  /build/postgresql.tar.bz2' | sha256sum -c -; \
    tar -xjf /build/postgresql.tar.bz2 -C /build; \
    cd /build/postgresql-18.7; \
    ./configure --prefix=/opt/pg-smoke --with-icu --with-ssl=openssl --without-gssapi --without-ldap --without-libxml --without-libxslt; \
    make -j2; \
    make install; \
    /opt/pg-smoke/bin/pg_config --configure > /smoke-build-evidence/configure.txt; \
    /opt/pg-smoke/bin/pg_config --version > /smoke-build-evidence/version.txt

FROM mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452
COPY --from=build /opt/pg-smoke /opt/pg-smoke
COPY --from=build /smoke-build-evidence /smoke-build-evidence
