#!/usr/bin/env python3
"""Positive and negative controls for package and loader assertions."""

import unittest

from identity import ROOT_CONFIG, SHARED_LAYER, STOCK_CONFIG, validate_manifests
from validate import (
    parse_libraries,
    parse_live_packages,
    parse_reference_packages,
    validate_image_metadata,
    validate_libraries,
    validate_packages,
)


class ClosureControls(unittest.TestCase):
    def setUp(self):
        self.base = {
            "libc6": "2.41-12+deb13u4",
            "libssl3t64": "3.5.7-1~deb13u3",
            "zlib1g": "1:1.3.dfsg+really1.3.1-1+b1",
            "util-linux": "2.41.5-0+deb13u1",
            "libmount1": "2.41.5-0+deb13u1",
            "bsdutils": "1:2.41.5-0+deb13u1",
        }
        additions = {
            "libicu76": "76.1-4",
            "libc-l10n": "2.41-12+deb13u4",
            "locales": "2.41-12+deb13u4",
        }
        extras = {"libxml2": "1", "libgssapi-krb5-2": "1", "libkrb5-3": "1", "libldap2": "1"}
        self.stock = {**self.base, **additions, **extras}
        self.candidate = {**self.base, **additions}
        self.essential = {"libc.so.6", "libssl.so.3", "libcrypto.so.3", "libz.so.1", "libzstd.so.1"}
        self.icu = {"libicuuc.so.76", "libicui18n.so.76", "libicudata.so.76"}
        self.extra = {"libxml2.so.2", "libgssapi_krb5.so.2", "libldap.so.2"}

    def test_image_identity_controls(self):
        root = {"config": {"digest": ROOT_CONFIG}, "layers": [{"digest": SHARED_LAYER}]}
        stock = {"config": {"digest": STOCK_CONFIG}, "layers": [{"digest": SHARED_LAYER}]}
        validate_manifests(root, stock)
        for bad in ({}, {"config": {"digest": ROOT_CONFIG}, "layers": [{"digest": "wrong"}]}):
            with self.assertRaises(ValueError):
                validate_manifests(bad, stock)
        with self.assertRaises(ValueError):
            validate_manifests(root, {})

    def test_image_metadata_controls(self):
        base = {"Os": "linux", "Architecture": "amd64", "RootFS": {"Layers": ["base"]}}
        stock = {**base, "RootFS": {"Layers": ["base", "stock"]}}
        candidate = {
            **base,
            "RootFS": {"Layers": ["base", "candidate"]},
            "Config": {"Env": ["LANG=en_US.utf8"]},
        }
        validate_image_metadata(base, stock, candidate)
        with self.assertRaises(ValueError):
            validate_image_metadata(base, stock, {**candidate, "Architecture": "arm64"})
        with self.assertRaises(ValueError):
            validate_image_metadata(base, stock, {**candidate, "RootFS": {"Layers": ["other"]}})
        with self.assertRaises(ValueError):
            validate_image_metadata(base, stock, {**candidate, "Config": {"Env": []}})

    def test_positive_and_changed_packages(self):
        validate_packages(self.base, self.base, self.stock, self.candidate)
        for bad in (
            {},
            {**self.candidate, "libxml2": "1"},
            {**self.candidate, "libicu76": "wrong"},
        ):
            with self.assertRaises(ValueError):
                validate_packages(self.base, self.base, self.stock, bad)
        with self.assertRaises(ValueError):
            validate_packages(self.base, self.base, self.candidate, self.candidate)
        with self.assertRaises(ValueError):
            validate_packages({}, self.base, self.stock, self.candidate)

    def test_positive_and_changed_libraries(self):
        validate_libraries(
            self.essential, self.essential | self.icu | self.extra, self.essential | self.icu
        )
        for bad in (set(), self.essential, self.essential | self.icu | self.extra):
            with self.assertRaises(ValueError):
                validate_libraries(self.essential, self.essential | self.icu | self.extra, bad)
        with self.assertRaises(ValueError):
            validate_libraries(self.essential, self.essential | self.icu, self.essential | self.icu)

    def test_reference_and_live_parsers_are_distinct(self):
        self.assertEqual(parse_reference_packages("libc6:amd64\t2\n"), {"libc6": "2"})
        self.assertEqual(
            parse_live_packages("libc6:amd64\t2\tinstall ok installed\n"), {"libc6": "2"}
        )
        self.assertEqual(
            parse_libraries(" libz.so.1 (libc6,x86-64) => /lib/libz.so.1\n"), {"libz.so.1"}
        )
        for text in ("", "bad", "libc6\t2\nlibc6:amd64\t2\n", "libc6\t2\tinstall ok installed\n"):
            with self.assertRaises(ValueError):
                parse_reference_packages(text)
        for text in (
            "",
            "libc6\t2\n",
            "libc6\t2\tinstall ok installed\nlibc6:amd64\t2\tdeinstall ok config-files\n",
        ):
            with self.assertRaises(ValueError):
                parse_live_packages(text)
        with self.assertRaises(ValueError):
            parse_libraries("libz.so.1 => not found")

    def test_config_only_is_not_counted_as_package_presence(self):
        def inventory(packages, residual):
            return "".join(
                f"{name}\t{version}\t{'deinstall ok config-files' if name == residual else 'install ok installed'}\n"
                for name, version in packages.items()
            )

        stock = parse_live_packages(inventory(self.stock, "libxml2"))
        self.assertNotIn("libxml2", stock)
        self.assertEqual({**stock, "libxml2": "1"}, self.stock)
        with self.assertRaises(ValueError):
            validate_packages(self.base, self.base, stock, self.candidate)
        candidate = parse_live_packages(inventory(self.candidate, "locales"))
        self.assertNotIn("locales", candidate)
        self.assertEqual({**candidate, "locales": "2.41-12+deb13u4"}, self.candidate)
        with self.assertRaises(ValueError):
            validate_packages(self.base, self.base, self.stock, candidate)

    def test_partial_broken_and_invalid_states_fail_closed(self):
        for status in (
            "install ok half-installed",
            "install ok half-configured",
            "install ok unpacked",
            "install ok triggers-awaited",
            "install ok triggers-pending",
            "install reinstreq installed",
            "bogus ok installed",
            "install ok unknown",
            "installed",
            "deinstall ok installed",
        ):
            with self.subTest(status=status), self.assertRaises(ValueError):
                parse_live_packages(f"libc6\t2\tinstall ok installed\nlibxml2\t1\t{status}\n")
        self.assertEqual(
            parse_live_packages("libc6\t2\thold ok installed\nold\t\tunknown ok not-installed\n"),
            {"libc6": "2"},
        )


if __name__ == "__main__":
    unittest.main()
