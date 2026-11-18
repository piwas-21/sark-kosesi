# Disposable composition of two independently checked local experiment images.
ARG SOURCE_IMAGE
ARG SIBLING_IMAGE
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
