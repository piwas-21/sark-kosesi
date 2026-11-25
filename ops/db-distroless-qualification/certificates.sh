#!/usr/bin/env bash
# Disposable synthetic fixture; private inputs stay in runner scratch.
set +x
set -euo pipefail
umask 077
private=${1:?}
[[ -d "$private" && ! -L "$private" ]]
[[ "$(stat -c '%u:%a' "$private")" == "$(id -u):700" ]]
# Independent throwaway CAs and a single-DNS-SAN server certificate.
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 1 -subj '/CN=TLS Lab CA' \
  -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -keyout "$private/ca.key" -out "$private/ca.crt" > "$private/openssl.log" 2>&1
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 1 -subj '/CN=TLS Lab Wrong CA' \
  -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -keyout "$private/wrong-ca.key" -out "$private/wrong-ca.crt" >> "$private/openssl.log" 2>&1
openssl req -new -newkey rsa:2048 -nodes -sha256 -subj '/CN=sibling-db.test' \
  -keyout "$private/server.key" -out "$private/server.csr" >> "$private/openssl.log" 2>&1
printf '%s\n' 'basicConstraints=critical,CA:FALSE' 'keyUsage=critical,digitalSignature,keyEncipherment' \
  'extendedKeyUsage=serverAuth' 'subjectAltName=DNS:sibling-db.test' > "$private/server.ext"
openssl x509 -req -sha256 -days 1 -set_serial 1 -in "$private/server.csr" -CA "$private/ca.crt" \
  -CAkey "$private/ca.key" -extfile "$private/server.ext" -out "$private/server.crt" >> "$private/openssl.log" 2>&1
openssl verify -purpose sslserver -CAfile "$private/ca.crt" "$private/server.crt" > "$private/verify.log" 2>&1
if openssl verify -purpose sslserver -CAfile "$private/wrong-ca.crt" "$private/server.crt" > "$private/wrong-verify.log" 2>&1; then exit 1; fi
openssl rand -hex 32 > "$private/password"
rm -- "$private/ca.key" "$private/wrong-ca.key" "$private/server.csr"
