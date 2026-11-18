# Disposable feature experiment. The retained stock root filesystem is not a release candidate.
FROM mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452 AS build
RUN set -eu; \
    apt-get update; \
    apt-get install -y --no-install-recommends build-essential bison bzip2 ca-certificates curl flex libicu-dev libssl-dev zlib1g-dev libreadline-dev pkgconf; \
    mkdir -p /build /smoke-build-evidence; \
    dpkg-query -W -f='${binary:Package}=${Version}\n' | LC_ALL=C sort > /smoke-build-evidence/build-packages.txt; \
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
