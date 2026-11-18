#!/usr/bin/env bash
set -euo pipefail

# All names and credentials belong to this run. No ports or persistent volumes are published.
[[ ${ANALYSIS_REVISION:-} =~ ^[0-9a-f]{40}$ ]]
[[ $(git rev-parse HEAD) == "$ANALYSIS_REVISION" ]]
run_id=$(python3 -c 'import secrets; print(secrets.token_hex(8))')
token=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
password=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
network="sark-passive-$run_id"
export DOCKER_CONFIG="$PWD/.local/docker-$run_id"
mkdir -p "$DOCKER_CONFIG"
docker() { command docker --host unix:///var/run/docker.sock "$@"; }
[[ $(docker info --format '{{.OSType}}/{{.Architecture}}') == linux/x86_64 ]]
db="$network-db"
web="$network-web"
api="$network-api"
scanner="$network-zap"
preflight="$network-preflight"
seed_image="$network-seed"
api_image="$network-api-image"
web_image="$network-web-image"
zap_image='ghcr.io/zaproxy/zaproxy@sha256:c2d63bbb2d907a5c5257914fb20e26e969c92b25f265def7f89f08203f912882'
postgres_image='postgres:18.7-trixie@sha256:9f4fd4aa918f52ae3d6f3b687f1de90d8f867cd4d520021545a9a50c120eae3b'
cleanup() {
  docker rm -fv "$preflight" "$scanner" "$api" "$web" "$db" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  docker image rm "$seed_image" "$api_image" "$web_image" >/dev/null 2>&1 || true
  rm -rf -- "$DOCKER_CONFIG"
}
scratch=$(mktemp -d)
cleanup_all() { rm -rf -- "$scratch"; cleanup; }
trap cleanup_all EXIT
prepare_certificate() {
  local name=$1
  openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj '/CN=127.0.0.1' \
    -addext 'subjectAltName=IP:127.0.0.1' -keyout "$scratch/key.pem" \
    -out "$scratch/cert.pem" >/dev/null 2>&1
  docker cp "$scratch/key.pem" "$name:/tmp/fixture-key.pem"
  docker cp "$scratch/cert.pem" "$name:/tmp/fixture-cert.pem"
  docker exec --user 0 "$name" chown 1000:1000 /tmp/fixture-key.pem /tmp/fixture-cert.pem
  rm -f -- "$scratch/key.pem" "$scratch/cert.pem"
}

# Build and pull before creating the internal-only runtime network.
docker pull --platform linux/amd64 "$zap_image" >/dev/null
[[ $(docker image inspect --format '{{.Os}}/{{.Architecture}}' "$zap_image") == linux/amd64 ]]
# A pinned-scanner loopback rehearsal detects API/history incompatibilities before builds.
docker run -d --platform linux/amd64 --name "$preflight" --network none \
  --env "PREVIEW_TOKEN=$token" --env "ANALYSIS_REVISION=$ANALYSIS_REVISION" \
  --mount "type=bind,src=$PWD/tests/passive-preview,dst=/assessment,readonly" \
  --entrypoint /zap/zap.sh "$zap_image" -daemon -host 127.0.0.1 -port 8080 \
  -config api.disablekey=true -config pscans.maxBodySizeInBytes=0 >/dev/null
prepare_certificate "$preflight"
docker exec "$preflight" python3 /assessment/preflight.py
docker rm -fv "$preflight" >/dev/null
docker pull "$postgres_image" >/dev/null
docker build -f tests/passive-preview/Dockerfile.seed -t "$seed_image" .
docker build -f deploy/staging/Dockerfile --target api --build-arg "SOURCE_REVISION=$ANALYSIS_REVISION" -t "$api_image" .
docker build -f deploy/staging/Dockerfile --target web --build-arg "SOURCE_REVISION=$ANALYSIS_REVISION" -t "$web_image" .
docker network create --internal "$network" >/dev/null
[[ $(docker network inspect --format '{{.Internal}}' "$network") == true ]]
docker run -d --platform linux/amd64 --name "$db" --network "$network" --network-alias db \
  --tmpfs /var/lib/postgresql:rw --env POSTGRES_USER=preview --env POSTGRES_DB=preview \
  --env "POSTGRES_PASSWORD=$password" "$postgres_image" >/dev/null
docker inspect "$db" | python3 -c 'import json,sys; d=json.load(sys.stdin)[0]; assert "/var/lib/postgresql" in d["HostConfig"]["Tmpfs"]; assert all(m["Type"] != "volume" for m in d["Mounts"]); assert not d["HostConfig"]["PortBindings"]'
for attempt in $(seq 1 60); do
  if docker exec "$db" pg_isready -h 127.0.0.1 -U preview -d preview >/dev/null 2>&1; then break; fi
  [[ $attempt -lt 60 ]] || { echo 'Disposable database did not become ready'; exit 1; }
  sleep 1
done
database_url="postgresql://preview:$password@db:5432/preview"
docker run --rm --platform linux/amd64 --network "$network" --env APP_ENV=test --env "DATABASE_URL=$database_url" \
  --env "PREVIEW_TOKEN=$token" "$seed_image"
docker run -d --platform linux/amd64 --name "$web" --network "$network" --env APP_ENV=staging \
  --env "PREVIEW_REVISION=$ANALYSIS_REVISION" --env "PREVIEW_TOKEN=$token" \
  --env API_URL=http://127.0.0.1:45671 --env CANONICAL_ORIGIN=https://preview.invalid \
  "$web_image" >/dev/null
docker run -d --platform linux/amd64 --name "$api" --network "container:$web" --env APP_ENV=staging \
  --env API_BIND=127.0.0.1:45671 --env "DATABASE_URL=$database_url" \
  --env "PREVIEW_TOKEN=$token" "$api_image" >/dev/null
for attempt in $(seq 1 90); do
  if docker exec "$web" node -e 'fetch("http://127.0.0.1:45671/health/ready").then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))' >/dev/null 2>&1; then break; fi
  [[ $attempt -lt 90 ]] || { echo 'Preview API did not become ready'; exit 1; }
  sleep 1
done
for attempt in $(seq 1 90); do
  if docker exec "$web" node -e 'const auth="Basic "+Buffer.from("preview:"+process.env.PREVIEW_TOKEN).toString("base64");fetch("http://127.0.0.1:3000/nl/room-planner",{headers:{Authorization:auth}}).then(r=>{if(!r.ok)process.exit(1)}).catch(()=>process.exit(1))' >/dev/null 2>&1; then break; fi
  [[ $attempt -lt 90 ]] || { echo 'Preview web did not become ready'; exit 1; }
  sleep 1
done
# Sharing the app's internal-only network namespace keeps its API loopback-only.
docker run -d --platform linux/amd64 --name "$scanner" --network "container:$web" \
  --env "PREVIEW_TOKEN=$token" --env "ANALYSIS_REVISION=$ANALYSIS_REVISION" \
  --mount "type=bind,src=$PWD/tests/passive-preview,dst=/assessment,readonly" \
  --entrypoint /zap/zap.sh "$zap_image" -daemon -host 127.0.0.1 -port 8080 \
  -config api.disablekey=true -config pscans.maxBodySizeInBytes=0 >/dev/null
prepare_certificate "$scanner"
mkdir -p .local/evidence
report=$(mktemp)
trap 'rm -f "$report"; cleanup_all' EXIT
if ! docker exec "$scanner" python3 /assessment/run_scan.py >"$report"; then
  cat "$report"
  echo 'Passive scanner or control failed; no assessment evidence retained.'
  exit 1
fi
cp "$report" .local/evidence/passive-preview.json
cat "$report"
