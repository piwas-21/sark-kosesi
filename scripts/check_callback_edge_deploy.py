#!/usr/bin/env python3
"""Check the optional edge source policy; this cannot attest a running proxy."""

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OVERLAY = ROOT / "deploy/staging/callback-edge.compose.yml"
ROUTER = "traefik.http.routers.sark-callback-edge."
MIDDLEWARE = "traefik.http.middlewares."
SERVICE = "traefik.http.services.sark-callback-edge."
HEALTH_PROBE = (
    "fetch('http://127.0.0.1:45672/health/live',{signal:AbortSignal.timeout(1000)})"
    ".then(r=>{if(r.status!==204)process.exit(1)}).catch(()=>process.exit(1))"
)


def required_reference(value, variable):
    return isinstance(value, str) and re.fullmatch(r"\$\{" + variable + r":\?[^}]+\}", value)


def bounded_integer(value, maximum, minimum=1):
    return (
        isinstance(value, str)
        and len(value) <= 10
        and re.fullmatch(r"[0-9]+", value)
        and minimum <= int(value) <= maximum
    )


def findings(document):
    """Return static categories, never environment or credential values."""
    issues = []

    def require(condition, message):
        if not condition:
            issues.append(message)

    if not isinstance(document, dict) or set(document) != {"services"}:
        return ["Overlay must only add its two service overrides"]
    services = document["services"]
    if not isinstance(services, dict) or set(services) != {"api", "callback-edge"}:
        return ["Overlay must not alter web, database, networks or other services"]
    api = services["api"]
    require(
        isinstance(api, dict)
        and set(api) == {"environment"}
        and isinstance(api["environment"], dict)
        and set(api["environment"]) == {"CALLBACK_INTAKE_PROFILE_ID"}
        and required_reference(
            api["environment"]["CALLBACK_INTAKE_PROFILE_ID"], "CALLBACK_INTAKE_PROFILE_ID"
        ),
        "API override may only require the configured intake profile",
    )
    edge = services["callback-edge"]
    if not isinstance(edge, dict):
        return [*issues, "Edge service must be an object"]
    allowed = {
        "profiles",
        "image",
        "user",
        "environment",
        "network_mode",
        "depends_on",
        "healthcheck",
        "labels",
        "restart",
        "read_only",
        "tmpfs",
        "mem_limit",
        "cpus",
        "pids_limit",
        "cap_drop",
        "security_opt",
    }
    require(set(edge) == allowed, "Edge must not expose ports, mounts or additional privileges")
    require(edge.get("profiles") == ["callback-edge"], "Edge must be an optional profile")
    image = edge.get("image", "")
    require(
        isinstance(image, str)
        and image.startswith("sark-kosesi-callback-edge:")
        and required_reference(
            image.removeprefix("sark-kosesi-callback-edge:"), "STAGING_REVISION"
        ),
        "Edge image must require a reviewed revision",
    )
    require(edge.get("network_mode") == "service:web", "Edge must share the private API namespace")
    require(
        edge.get("user") == "1000:1000" and edge.get("read_only") is True,
        "Edge must be an unprivileged read-only process",
    )
    require(
        edge.get("cap_drop") == ["ALL"] and edge.get("security_opt") == ["no-new-privileges:true"],
        "Edge capabilities must be dropped",
    )
    require(
        edge.get("tmpfs") == ["/tmp:size=16m,uid=1000,gid=1000"],
        "Writable storage must be bounded temporary storage",
    )
    require(edge.get("mem_limit") == "256m", "Edge memory must be capped")
    require(
        type(edge.get("cpus")) in (int, float) and 0 < edge["cpus"] <= 1, "Edge CPU must be capped"
    )
    require(
        type(edge.get("pids_limit")) is int and 0 < edge["pids_limit"] <= 128,
        "Edge process count must be capped",
    )
    require(
        edge.get("restart") == "unless-stopped", "Edge restart policy must preserve operator stops"
    )
    require(
        edge.get("depends_on")
        == {"web": {"condition": "service_started"}, "api": {"condition": "service_started"}},
        "Edge must start after its private namespace and API",
    )
    expected = {
        "APP_ENV": "staging",
        "CALLBACK_EDGE_ENABLED": "true",
        "CALLBACK_EDGE_BIND": "0.0.0.0:45672",
        "API_URL": "http://127.0.0.1:45671",
    }
    env = edge.get("environment")
    if not isinstance(env, dict):
        issues.append("Edge environment must be a closed mapping")
    else:
        require(
            set(env) == {*expected, "CALLBACK_EDGE_PROXY_IPS", "PREVIEW_TOKEN", "PREVIEW_REVISION"},
            "Edge must receive no database or provider credentials",
        )
        require(
            all(env.get(key) == value for key, value in expected.items()),
            "Edge must retain staging, fixed bind and loopback upstream",
        )
        for key, variable in (
            ("CALLBACK_EDGE_PROXY_IPS", "CALLBACK_EDGE_PROXY_IPS"),
            ("PREVIEW_TOKEN", "STAGING_PREVIEW_TOKEN"),
            ("PREVIEW_REVISION", "STAGING_REVISION"),
        ):
            require(
                required_reference(env.get(key), variable),
                "Edge configuration must not supply trusted defaults or credentials",
            )
    health = edge.get("healthcheck")
    require(
        isinstance(health, dict)
        and health.get("test") == ["CMD", "node", "-e", HEALTH_PROBE]
        and health.get("timeout") == "3s",
        "Health must be a bounded read-only process probe",
    )
    labels = edge.get("labels")
    if not isinstance(labels, dict) or not all(
        isinstance(k, str) and isinstance(v, str) for k, v in labels.items()
    ):
        return [*issues, "Traefik labels must be a string mapping"]
    route = {
        key.removeprefix(ROUTER): value for key, value in labels.items() if key.startswith(ROUTER)
    }
    require(
        set(route)
        == {"entrypoints", "rule", "priority", "service", "tls", "tls.certresolver", "middlewares"},
        "Only the exact HTTPS callback router is permitted",
    )
    require(
        labels.get("traefik.enable") == "${STAGING_CALLBACK_INGRESS_ENABLED:-false}",
        "Public callback routing must be independently disabled by default",
    )
    require(
        labels.get("traefik.docker.network") == "coolify",
        "Proxy network selection must remain explicit",
    )
    require(
        route.get("entrypoints") == "https"
        and route.get("tls") == "true"
        and route.get("tls.certresolver") == "letsencrypt",
        "Callback routing must require the configured TLS entrypoint",
    )
    require(
        re.fullmatch(
            r"Host\(`\$\{STAGING_HOST\}`\)\s*&&\s*Path\(`/api/callbacks/mollie`\)\s*&&\s*Method\(`POST`\)",
            route.get("rule", ""),
        ),
        "Only the exact host, callback path and POST may route anonymously",
    )
    require(
        bounded_integer(route.get("priority"), 10000, minimum=1000),
        "Callback priority must explicitly precede the base host router",
    )
    require(
        route.get("service") == "sark-callback-edge"
        and labels.get(SERVICE + "loadbalancer.server.port") == "45672",
        "Only the dedicated edge port may be routed",
    )
    require(
        route.get("middlewares", "").split(",")
        == [
            "sark-callback-rate",
            "sark-callback-group",
            "sark-callback-flight",
            "sark-callback-buffer",
        ],
        "Rate, overwritten group, inflight and buffering must apply in order",
    )
    rate = MIDDLEWARE + "sark-callback-rate.ratelimit."
    flight = MIDDLEWARE + "sark-callback-flight.inflightreq."
    group = MIDDLEWARE + "sark-callback-group.headers.customrequestheaders.x-piwas-callback-budget"
    buffer = MIDDLEWARE + "sark-callback-buffer.buffering."
    allowed_labels = {
        "traefik.enable",
        "traefik.docker.network",
        *(ROUTER + key for key in route),
        SERVICE + "loadbalancer.server.port",
        rate + "average",
        rate + "period",
        rate + "burst",
        group,
        flight + "amount",
        flight + "sourcecriterion.requestheadername",
        *(
            buffer + key
            for key in (
                "maxrequestbodybytes",
                "memrequestbodybytes",
                "maxresponsebodybytes",
                "memresponsebodybytes",
            )
        ),
    }
    require(
        set(labels) == allowed_labels,
        "No extra routers, forwarding trust, retries or labels are permitted",
    )
    require(
        bounded_integer(labels.get(rate + "average"), 30)
        and bounded_integer(labels.get(rate + "burst"), 10)
        and labels.get(rate + "period") in {"1m", "60s"},
        "Per-remote-address rate limits must remain bounded",
    )
    require(
        bounded_integer(labels.get(flight + "amount"), 16),
        "Global inflight requests must remain bounded",
    )
    require(
        labels.get(group) == "sark-callback"
        and labels.get(flight + "sourcecriterion.requestheadername", "").lower()
        == "x-piwas-callback-budget",
        "Inflight grouping must use the overwritten nonsensitive constant",
    )
    for direction in ("request", "response"):
        maximum = labels.get(buffer + "max" + direction + "bodybytes")
        memory = labels.get(buffer + "mem" + direction + "bodybytes")
        require(
            maximum == "1024" and memory == "1024",
            "Buffer limits must preserve the 1024-byte contract without spilling to disk",
        )
    require(
        not any(
            re.search(r"\$\{[^}]*(?:TOKEN|PASSWORD|SECRET|API_KEY)", value)
            for value in labels.values()
        ),
        "Credentials cannot enter labels",
    )
    return issues


def main():
    # JSON is a valid YAML subset, keeping the source check dependency-free.
    issues = findings(json.loads(OVERLAY.read_text()))
    if issues:
        raise ValueError("; ".join(issues))
    print(
        "Disabled callback-edge source policy passed; runtime proxy/TLS/provider acceptance remains pending"
    )


if __name__ == "__main__":
    main()
