# Disposable evidence candidate only; staging continues to use its existing pin.
FROM mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452

RUN --mount=type=bind,source=ca-certificates.crt,target=/tmp/zlib-candidate-ca.crt \
    --mount=type=bind,source=sanitize_apt_failure.sh,target=/tmp/sanitize-apt-failure.sh \
    set -eu; \
    evidence=/usr/local/share/db-zlib-candidate-evidence; \
    work=/tmp/zlib-candidate; \
    mkdir -p "$evidence" "$work"; \
    installed="$(dpkg-query -W -f='${Version}' zlib1g 2> "$work/dpkg-query.log")" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; \
    test "$installed" = '1:1.3.dfsg+really1.3.1-1+b1'; \
    dpkg-query -W -f='${binary:Package}=${Version}\n' > "$work/packages-before.txt" 2> "$work/dpkg-query.log" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; \
    LC_ALL=C sort "$work/packages-before.txt" > "$evidence/packages-before.txt"; \
    printf '%s\n' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://security.debian.org/debian-security trixie-security main' > "$evidence/security.sources.list"; \
    printf 'ca_bundle_sha256=%s\ndebian_keyring_sha256=%s\n' "$(sha256sum /tmp/zlib-candidate-ca.crt | cut -d ' ' -f 1)" "$(sha256sum /usr/share/keyrings/debian-archive-keyring.gpg | cut -d ' ' -f 1)" > "$evidence/trust-sha256.txt"; \
    apt-get -o Acquire::https::CAInfo=/tmp/zlib-candidate-ca.crt -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- -o Acquire::AllowInsecureRepositories=false -o Acquire::AllowDowngradeToInsecureRepositories=false -o APT::Get::AllowUnauthenticated=false -o APT::Update::Error-Mode=any -o Debug::pkgAcquire::Auth=true update > "$work/apt-update.log" 2>&1 || { rc=$?; sh /tmp/sanitize-apt-failure.sh apt_update "$rc" "$work/apt-update.log"; exit "$rc"; }; \
    printf '%s\n' 'stage=apt_update category=success exit=0' > "$evidence/apt-status.txt"; \
    apt-cache -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- show 'zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1' > "$work/package-metadata.txt" 2> "$work/apt-cache.log" || { rc=$?; sh /tmp/sanitize-apt-failure.sh apt_cache "$rc" "$work/apt-cache.log"; exit "$rc"; }; \
    printf '%s\n' 'stage=apt_cache category=success exit=0' >> "$evidence/apt-status.txt"; \
    test "$(awk '/^SHA256: / {print $2}' "$work/package-metadata.txt" | sort -u | wc -l)" -eq 1; \
    expected="$(awk '/^SHA256: / {print $2}' "$work/package-metadata.txt" | sort -u)"; \
    test "$expected" = 8c27c342230503b73dfde629642c3b352e87b554e06d60cf58772f236a305ceb; \
    cd "$work"; \
    apt-get -o Acquire::https::CAInfo=/tmp/zlib-candidate-ca.crt -o Dir::Etc::sourcelist="$evidence/security.sources.list" -o Dir::Etc::sourceparts=- -o APT::Get::AllowUnauthenticated=false download 'zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1' > "$work/apt-download.log" 2>&1 || { rc=$?; sh /tmp/sanitize-apt-failure.sh apt_download "$rc" "$work/apt-download.log"; exit "$rc"; }; \
    printf '%s\n' 'stage=apt_download category=success exit=0' >> "$evidence/apt-status.txt"; \
    set -- ./*.deb; test "$#" -eq 1; test -f "$1"; \
    actual="$(sha256sum "$1" | cut -d ' ' -f 1)"; test "$actual" = "$expected"; \
    package="$(dpkg-deb -f "$1" Package 2> "$work/dpkg-query.log")" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; test "$package" = zlib1g; \
    version="$(dpkg-deb -f "$1" Version 2> "$work/dpkg-query.log")" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; test "$version" = '1:1.3.dfsg+really1.3.1-1+deb13u1'; \
    arch="$(dpkg-deb -f "$1" Architecture 2> "$work/dpkg-query.log")" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; test "$arch" = amd64; \
    printf '%s  %s\n' "$actual" package.deb > "$evidence/package-sha256.txt"; \
    cp "$1" "$evidence/package.deb"; \
    for f in /var/lib/apt/lists/*_InRelease /var/lib/apt/lists/*security*binary-amd64_Packages*; do test -f "$f"; cp "$f" "$evidence/"; done; \
    dpkg -i "$1" > "$work/dpkg-install.log" 2>&1 || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-install.log"; exit "$rc"; }; \
    printf '%s\n' 'stage=dpkg_install category=success exit=0' >> "$evidence/apt-status.txt"; \
    installed="$(dpkg-query -W -f='${Version}' zlib1g 2> "$work/dpkg-query.log")" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; \
    test "$installed" = '1:1.3.dfsg+really1.3.1-1+deb13u1'; \
    dpkg-query -W -f='${binary:Package}=${Version}\n' > "$work/packages-after.txt" 2> "$work/dpkg-query.log" || { rc=$?; sh /tmp/sanitize-apt-failure.sh dpkg_install "$rc" "$work/dpkg-query.log"; exit "$rc"; }; \
    LC_ALL=C sort "$work/packages-after.txt" > "$evidence/packages-after.txt"; \
    sed '/^zlib1g\(:amd64\)\?=/d' "$evidence/packages-before.txt" > "$work/before-other"; \
    sed '/^zlib1g\(:amd64\)\?=/d' "$evidence/packages-after.txt" > "$work/after-other"; \
    cmp "$work/before-other" "$work/after-other"; \
    rm -rf "$work" /var/lib/apt/lists/*
