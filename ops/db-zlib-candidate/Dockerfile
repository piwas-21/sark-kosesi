# Disposable evidence candidate only; staging continues to use its existing pin.
FROM mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452

RUN set -eu; \
    evidence=/usr/local/share/db-zlib-candidate-evidence; \
    mkdir -p "$evidence" /tmp/zlib-candidate; \
    test "$(dpkg-query -W -f='${Version}' zlib1g)" = '1:1.3.dfsg+really1.3.1-1+b1'; \
    dpkg-query -W -f='${binary:Package}=${Version}\n' | LC_ALL=C sort > "$evidence/packages-before.txt"; \
    printf '%s\n' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://security.debian.org/debian-security trixie-security main' > "$evidence/security.sources.list"; \
    apt-get -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- -o Acquire::AllowInsecureRepositories=false -o Acquire::AllowDowngradeToInsecureRepositories=false -o APT::Get::AllowUnauthenticated=false -o APT::Update::Error-Mode=any -o Debug::pkgAcquire::Auth=true update > "$evidence/apt-update.log" 2>&1; \
    apt-cache -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- policy zlib1g > "$evidence/apt-policy.txt"; \
    apt-cache -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- show 'zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1' > "$evidence/package-metadata.txt"; \
    test "$(awk '/^SHA256: / {print $2}' "$evidence/package-metadata.txt" | sort -u | wc -l)" -eq 1; \
    expected="$(awk '/^SHA256: / {print $2}' "$evidence/package-metadata.txt" | sort -u)"; \
    test "$expected" = 8c27c342230503b73dfde629642c3b352e87b554e06d60cf58772f236a305ceb; \
    cd /tmp/zlib-candidate; \
    apt-get -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- -o APT::Get::AllowUnauthenticated=false download 'zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1' > "$evidence/apt-download.log" 2>&1; \
    set -- ./*.deb; test "$#" -eq 1; test -f "$1"; \
    test "$(dpkg-deb -f "$1" Package)" = zlib1g; \
    test "$(dpkg-deb -f "$1" Version)" = '1:1.3.dfsg+really1.3.1-1+deb13u1'; \
    test "$(dpkg-deb -f "$1" Architecture)" = amd64; \
    actual="$(sha256sum "$1" | cut -d ' ' -f 1)"; test "$actual" = "$expected"; \
    printf '%s  %s\n' "$actual" "$(basename "$1")" > "$evidence/package-sha256.txt"; \
    cp "$1" "$evidence/package.deb"; \
    cp /usr/share/keyrings/debian-archive-keyring.gpg "$evidence/debian-archive-keyring.gpg"; \
    for f in /var/lib/apt/lists/*_InRelease /var/lib/apt/lists/*security*binary-amd64_Packages*; do test -f "$f"; cp "$f" "$evidence/"; done; \
    dpkg -i "$1" > "$evidence/dpkg-install.log" 2>&1; \
    test "$(dpkg-query -W -f='${Version}' zlib1g)" = '1:1.3.dfsg+really1.3.1-1+deb13u1'; \
    dpkg-query -W -f='${binary:Package}=${Version}\n' | LC_ALL=C sort > "$evidence/packages-after.txt"; \
    sed '/^zlib1g\(:amd64\)\?=/d' "$evidence/packages-before.txt" > /tmp/before-other; \
    sed '/^zlib1g\(:amd64\)\?=/d' "$evidence/packages-after.txt" > /tmp/after-other; \
    cmp /tmp/before-other /tmp/after-other; \
    rm -rf /tmp/zlib-candidate /tmp/before-other /tmp/after-other /var/lib/apt/lists/*
