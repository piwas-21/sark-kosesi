import copy
import io
import json
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

import check_callback_edge_deploy as policy


class CallbackEdgeDeployment(unittest.TestCase):
    def setUp(self):
        self.document = json.loads(policy.OVERLAY.read_text())

    def changed(self, *path, value):
        candidate = copy.deepcopy(self.document)
        selected = candidate
        for key in path[:-1]:
            selected = selected[key]
        selected[path[-1]] = value
        return candidate

    def test_committed_policy_and_stricter_resource_controls_are_healthy(self):
        self.assertEqual(policy.findings(self.document), [])
        stricter = copy.deepcopy(self.document)
        edge = stricter["services"]["callback-edge"]
        edge["cpus"] = 0.25
        edge["pids_limit"] = 32
        labels = edge["labels"]
        labels[policy.ROUTER + "priority"] = "2000"
        labels[policy.MIDDLEWARE + "sark-callback-rate.ratelimit.average"] = "15"
        labels[policy.MIDDLEWARE + "sark-callback-rate.ratelimit.period"] = "60s"
        labels[policy.MIDDLEWARE + "sark-callback-rate.ratelimit.burst"] = "5"
        labels[policy.MIDDLEWARE + "sark-callback-flight.inflightreq.amount"] = "8"
        self.assertEqual(policy.findings(stricter), [])

    def test_unclosed_overlay_and_other_service_mutations_fail_closed(self):
        for candidate in (None, [], {}, {"services": {}}, {"services": []}):
            with self.subTest(candidate=candidate):
                self.assertTrue(policy.findings(candidate))
        for path, value in (
            (("networks",), {}),
            (("services", "web"), {"labels": {"traefik.enable": "true"}}),
            (("services", "callback-edge"), None),
            (("services", "api"), []),
            (("services", "api", "environment"), {"DATABASE_URL": "fixture"}),
            (("services", "api", "environment", "CALLBACK_INTAKE_PROFILE_ID"), "pfl_guessed"),
        ):
            with self.subTest(path=path):
                self.assertTrue(policy.findings(self.changed(*path, value=value)))

    def test_network_privilege_and_resource_mutations_are_detected(self):
        for field, value in (
            ("profiles", []),
            ("image", "sark-kosesi-callback-edge:latest"),
            ("network_mode", "host"),
            ("ports", ["45672:45672"]),
            ("volumes", ["/var/run/docker.sock:/var/run/docker.sock"]),
            ("privileged", True),
            ("user", "0:0"),
            ("read_only", False),
            ("cap_drop", []),
            ("security_opt", []),
            ("tmpfs", ["/tmp"]),
            ("mem_limit", "2g"),
            ("cpus", 8),
            ("cpus", True),
            ("pids_limit", 512),
            ("pids_limit", False),
            ("restart", "always"),
            ("depends_on", {}),
        ):
            with self.subTest(field=field, value=value):
                self.assertTrue(
                    policy.findings(self.changed("services", "callback-edge", field, value=value))
                )

    def test_environment_has_no_guessed_peers_or_unnecessary_credentials(self):
        for field, value in (
            ("APP_ENV", "production"),
            ("CALLBACK_EDGE_ENABLED", "false"),
            ("CALLBACK_EDGE_BIND", "0.0.0.0:80"),
            ("API_URL", "https://elsewhere.invalid"),
            ("CALLBACK_EDGE_PROXY_IPS", "172.20.0.2"),
            ("PREVIEW_TOKEN", "committed-placeholder"),
            ("PREVIEW_REVISION", "${STAGING_REVISION:-fallback}"),
            ("DATABASE_URL", "fixture"),
            ("MOLLIE_TEST_API_KEY", "fixture"),
        ):
            with self.subTest(field=field):
                self.assertTrue(
                    policy.findings(
                        self.changed("services", "callback-edge", "environment", field, value=value)
                    )
                )
        self.assertTrue(
            policy.findings(self.changed("services", "callback-edge", "environment", value=[]))
        )

    def test_public_router_is_default_off_exact_post_tls_and_only_edge_port(self):
        for field, value in (
            ("traefik.enable", "true"),
            ("traefik.docker.network", "other"),
            (policy.ROUTER + "entrypoints", "http,https"),
            (policy.ROUTER + "rule", "Host(`${STAGING_HOST}`) && PathPrefix(`/api`)"),
            (
                policy.ROUTER + "rule",
                "Host(`${STAGING_HOST}`) && Path(`/api/callbacks/mollie`) && Method(`GET`)",
            ),
            (policy.ROUTER + "rule", "Host(`${STAGING_HOST}`) || Path(`/api/callbacks/mollie`)"),
            (policy.ROUTER + "priority", "1"),
            (policy.ROUTER + "service", "sark-preview"),
            (policy.ROUTER + "tls", "false"),
            (policy.ROUTER + "tls.certresolver", ""),
            (policy.SERVICE + "loadbalancer.server.port", "3000"),
            ("traefik.http.routers.other.rule", "PathPrefix(`/`)"),
        ):
            with self.subTest(field=field):
                self.assertTrue(
                    policy.findings(
                        self.changed("services", "callback-edge", "labels", field, value=value)
                    )
                )

    def test_middleware_controls_reject_spoofed_groups_unbounded_buffers_and_retries(self):
        base = policy.MIDDLEWARE
        for field, value in (
            (
                policy.ROUTER + "middlewares",
                "sark-callback-buffer,sark-callback-flight,sark-callback-rate",
            ),
            (base + "sark-callback-rate.ratelimit.average", "31"),
            (base + "sark-callback-rate.ratelimit.burst", "11"),
            (base + "sark-callback-rate.ratelimit.period", "1h"),
            (base + "sark-callback-rate.ratelimit.sourcecriterion.ipstrategy.depth", "1"),
            (
                base + "sark-callback-group.headers.customrequestheaders.x-piwas-callback-budget",
                "${CALLER_VALUE}",
            ),
            (base + "sark-callback-flight.inflightreq.amount", "17"),
            (
                base + "sark-callback-flight.inflightreq.sourcecriterion.requestheadername",
                "X-Forwarded-For",
            ),
            (base + "sark-callback-buffer.buffering.maxrequestbodybytes", "0"),
            (base + "sark-callback-buffer.buffering.maxrequestbodybytes", "1025"),
            (base + "sark-callback-buffer.buffering.memrequestbodybytes", "512"),
            (base + "sark-callback-buffer.buffering.maxresponsebodybytes", "0"),
            (base + "sark-callback-buffer.buffering.retryexpression", "IsNetworkError()"),
        ):
            with self.subTest(field=field, value=value):
                self.assertTrue(
                    policy.findings(
                        self.changed("services", "callback-edge", "labels", field, value=value)
                    )
                )

    def test_label_secret_and_malformed_mapping_detectors_have_positive_controls(self):
        self.assertEqual(policy.findings(self.document), [])
        candidate = self.changed(
            "services",
            "callback-edge",
            "labels",
            "private-credential",
            value="${STAGING_PREVIEW_TOKEN}",
        )
        self.assertIn("Credentials cannot enter labels", policy.findings(candidate))
        for value in ([], {"traefik.enable": True}, {1: "not-a-label"}):
            self.assertTrue(
                policy.findings(self.changed("services", "callback-edge", "labels", value=value))
            )
        self.assertTrue(
            policy.findings(self.changed("services", "callback-edge", "healthcheck", value={}))
        )
        self.assertTrue(
            policy.findings(
                self.changed(
                    "services",
                    "callback-edge",
                    "healthcheck",
                    "test",
                    value=["CMD", "node", "-e", "fetch('/api/callbacks/mollie',{method:'POST'})"],
                )
            )
        )
        for value in (None, "0", "-1", "1.0", "x", "9" * 100, "١"):
            self.assertFalse(policy.bounded_integer(value, 16))

    def test_source_command_reports_only_static_policy_categories(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "overlay.yml"
            path.write_text(json.dumps(self.document))
            with patch.object(policy, "OVERLAY", path), redirect_stdout(io.StringIO()) as stream:
                policy.main()
            self.assertIn(
                "runtime proxy/TLS/provider acceptance remains pending", stream.getvalue()
            )
            candidate = self.changed(
                "services",
                "callback-edge",
                "labels",
                "credential",
                value="${SECRET_FIXTURE_MARKER}",
            )
            path.write_text(json.dumps(candidate))
            with patch.object(policy, "OVERLAY", path), self.assertRaises(ValueError) as error:
                policy.main()
            self.assertNotIn("SECRET_FIXTURE_MARKER", str(error.exception))

    def test_docker_target_copies_the_built_edge_binary_with_pinned_ancestry(self):
        source = (policy.ROOT / "deploy/staging/Dockerfile").read_text()
        stage = source.split(" AS callback-edge\n", 1)[1].split("\nFROM ", 1)[0]
        self.assertIn(
            "COPY --from=rust-build /source/target/release/commerce-callback-edge /app/commerce-callback-edge",
            stage,
        )
        self.assertIn('ENTRYPOINT ["/app/commerce-callback-edge"]', stage)
        self.assertIn("USER node", stage)
        self.assertIn("RUN cargo build --release --locked -p commerce-api --bins", source)
        stages = set()
        for line in source.splitlines():
            if line.startswith("FROM "):
                _, parent, alias, stage_name = line.split()
                self.assertEqual(alias, "AS")
                if "@sha256:" in parent:
                    self.assertRegex(parent, r"@sha256:[a-f0-9]{64}$")
                else:
                    self.assertIn(parent, stages)
                self.assertNotIn(stage_name, stages)
                stages.add(stage_name)
