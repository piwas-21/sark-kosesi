# Experimental root only: no PostgreSQL server or deployment entrypoint.
FROM mirror.gcr.io/library/debian@sha256:7378260c0a098d27ac206225db5e7d92b8c7c84ea7afccc86eeb513756bbd392
COPY packages.lock /tmp/sibling-packages.lock
RUN --mount=type=bind,source=ca-certificates.crt,target=/tmp/runner-ca.crt \
    set -eu; \
    printf '%s\n' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg check-valid-until=no] https://snapshot.debian.org/archive/debian/20261118T000000Z/ trixie main' > /tmp/sibling-sources.list; \
    if [ -f /etc/dpkg/dpkg.cfg.d/docker ]; then sed -i '/\/usr\/share\/locale/d' /etc/dpkg/dpkg.cfg.d/docker; fi; \
    rm -rf /var/lib/apt/lists/*; \
    apt-get -o Acquire::https::CAInfo=/tmp/runner-ca.crt -o Dir::Etc::sourcelist=/tmp/sibling-sources.list -o Dir::Etc::sourceparts=- -o Acquire::AllowInsecureRepositories=false -o Acquire::AllowDowngradeToInsecureRepositories=false -o APT::Get::AllowUnauthenticated=false -o APT::Update::Error-Mode=any update; \
    xargs -r apt-get -y --no-install-recommends -o Acquire::https::CAInfo=/tmp/runner-ca.crt -o Dir::Etc::sourcelist=/tmp/sibling-sources.list -o Dir::Etc::sourceparts=- -o APT::Get::AllowUnauthenticated=false install < /tmp/sibling-packages.lock; \
    while IFS='=' read -r package version; do test "$(dpkg-query -W -f='${Version}' "$package")" = "$version"; done < /tmp/sibling-packages.lock; \
    echo 'en_US.UTF-8 UTF-8' >> /etc/locale.gen; \
    locale-gen; \
    locale -a | grep -Fx 'en_US.utf8'; \
    mkdir -p /usr/local/share/sibling-root-evidence; \
    sha256sum /usr/share/keyrings/debian-archive-keyring.gpg /tmp/runner-ca.crt /tmp/sibling-packages.lock > /usr/local/share/sibling-root-evidence/trust-inputs.txt; \
    rm -rf /var/lib/apt/lists/* /tmp/sibling-sources.list /tmp/sibling-packages.lock
ENV LANG=en_US.utf8
