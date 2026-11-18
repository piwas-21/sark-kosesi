# Disposable composition of two independently checked local experiment images.
ARG SOURCE_IMAGE
ARG SIBLING_IMAGE
ARG ZLIB_IMAGE
FROM ${ZLIB_IMAGE} AS zlib
FROM ${SOURCE_IMAGE} AS source
FROM ${SIBLING_IMAGE}
COPY packages.lock /tmp/compose-packages.lock
RUN --mount=type=bind,source=ca-certificates.crt,target=/tmp/runner-ca.crt \
    set -eu; \
    printf '%s\n' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg check-valid-until=no] https://snapshot.debian.org/archive/debian/20261118T000000Z/ trixie main' > /tmp/compose-sources.list; \
    rm -rf /var/lib/apt/lists/*; \
    apt-get -o Acquire::https::CAInfo=/tmp/runner-ca.crt -o Dir::Etc::sourcelist=/tmp/compose-sources.list -o Dir::Etc::sourceparts=- -o Acquire::AllowInsecureRepositories=false -o Acquire::AllowDowngradeToInsecureRepositories=false -o APT::Get::AllowUnauthenticated=false -o APT::Update::Error-Mode=any update; \
    xargs -r apt-get -y --no-install-recommends -o Acquire::https::CAInfo=/tmp/runner-ca.crt -o Dir::Etc::sourcelist=/tmp/compose-sources.list -o Dir::Etc::sourceparts=- -o APT::Get::AllowUnauthenticated=false install < /tmp/compose-packages.lock; \
    while IFS='=' read -r package version; do test "$(dpkg-query -W -f='${Version}' "$package")" = "$version"; done < /tmp/compose-packages.lock; \
    groupadd --system --gid 999 postgres; \
    useradd --system --uid 999 --gid 999 --home-dir /var/lib/postgresql --shell /bin/bash postgres; \
    mkdir -p /usr/local/share/sibling-postgres-evidence; \
    sha256sum /usr/share/keyrings/debian-archive-keyring.gpg /tmp/runner-ca.crt /tmp/compose-packages.lock > /usr/local/share/sibling-postgres-evidence/trust-inputs.txt; \
    rm -rf /var/lib/apt/lists/* /tmp/compose-sources.list /tmp/compose-packages.lock
COPY --from=source /opt/pg-smoke /opt/pg-smoke

# Consume the package authenticated and hash-checked by the existing zlib candidate.
RUN --mount=type=bind,from=zlib,source=/usr/local/share/db-zlib-candidate-evidence/package.deb,target=/tmp/zlib-security.deb \
    set -eu; \
    test "$(dpkg-query -W -f='${Version}' zlib1g)" = '1:1.3.dfsg+really1.3.1-1+b1'; \
    echo '8c27c342230503b73dfde629642c3b352e87b554e06d60cf58772f236a305ceb  /tmp/zlib-security.deb' | sha256sum -c -; \
    test "$(dpkg-deb -f /tmp/zlib-security.deb Package)" = zlib1g; \
    test "$(dpkg-deb -f /tmp/zlib-security.deb Version)" = '1:1.3.dfsg+really1.3.1-1+deb13u1'; \
    test "$(dpkg-deb -f /tmp/zlib-security.deb Architecture)" = amd64; \
    dpkg-query -W -f='${binary:Package}\t${Version}\t${Status}\n' | LC_ALL=C sort > /usr/local/share/sibling-postgres-evidence/zlib-before.txt; \
    dpkg -i /tmp/zlib-security.deb; \
    test "$(dpkg-query -W -f='${Version}' zlib1g)" = '1:1.3.dfsg+really1.3.1-1+deb13u1'; \
    test "$(dpkg-query -W -f='${Status}' zlib1g)" = 'install ok installed'; \
    dpkg-query -W -f='${binary:Package}\t${Version}\t${Status}\n' | LC_ALL=C sort > /usr/local/share/sibling-postgres-evidence/zlib-after.txt; \
    sed '/^zlib1g\(:amd64\)\?\t/d' /usr/local/share/sibling-postgres-evidence/zlib-before.txt > /tmp/before-other.txt; \
    sed '/^zlib1g\(:amd64\)\?\t/d' /usr/local/share/sibling-postgres-evidence/zlib-after.txt > /tmp/after-other.txt; \
    cmp /tmp/before-other.txt /tmp/after-other.txt; \
    test -z "$(dpkg --audit)"; \
    rm /tmp/before-other.txt /tmp/after-other.txt
