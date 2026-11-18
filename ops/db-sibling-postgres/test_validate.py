#!/usr/bin/env python3
"""Negative controls for this experiment's measured closure."""

import unittest

from validate import (
    check_image,
    check_linkage,
    check_loaded,
    check_packages,
    check_tree,
    check_zlib_builder,
    check_zlib_layer,
)


class CompositionControls(unittest.TestCase):
    def test_image_lineage(self):
        sibling = {"Os": "linux", "Architecture": "amd64", "RootFS": {"Layers": ["root", "extra"]}}
        candidate = {
            **sibling,
            "RootFS": {"Layers": ["root", "extra", "install"]},
            "Config": {"Env": ["LANG=en_US.utf8"]},
        }
        check_image(sibling, candidate)
        for bad in (
            {**candidate, "Architecture": "arm64"},
            {**candidate, "RootFS": {"Layers": ["other"]}},
            {**candidate, "Config": {"Env": []}},
        ):
            with self.assertRaises(ValueError):
                check_image(sibling, bad)

    def test_package_delta(self):
        sibling = {
            "libtinfo6": "6.5+20250216-2",
            "libicu76": "76.1-4",
            "zlib1g": "1:1.3.dfsg+really1.3.1-1+b1",
        }
        candidate = {
            **sibling,
            "libreadline8t64": "8.2-6",
            "readline-common": "8.2-6",
            "zlib1g": "1:1.3.dfsg+really1.3.1-1+deb13u1",
        }
        check_packages(sibling, candidate)
        for bad in (
            sibling,
            {**candidate, "libldap2": "1"},
            {**candidate, "zlib1g": "1:1.3.dfsg+really1.3.1-1+b1"},
            {**candidate, "zlib1g": "1:1.3.dfsg+really1.3.1-1+deb13u2"},
            {**candidate, "libreadline8t64": "wrong"},
        ):
            with self.assertRaises(ValueError):
                check_packages(sibling, bad)

    def test_zlib_source_image_is_a_single_layer_stock_derivative(self):
        stock_id = "sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d"
        package_id = "sha256:" + "a" * 64
        stock = {
            "Id": stock_id,
            "Os": "linux",
            "Architecture": "amd64",
            "RootFS": {"Layers": ["base"]},
            "Config": {"Cmd": ["postgres"]},
        }
        package = {**stock, "Id": package_id, "RootFS": {"Layers": ["base", "patch"]}}
        check_zlib_builder(stock, package, package_id)
        for bad in (
            {**package, "RootFS": {"Layers": ["other", "patch"]}},
            {**package, "Config": {"Cmd": ["changed"]}},
            {**package, "Id": stock_id},
        ):
            with self.assertRaises(ValueError):
                check_zlib_builder(stock, bad, package_id)

    def test_zlib_layer_only_changes_the_expected_installed_package(self):
        before = "libc6:amd64\t2.41\tinstall ok installed\nzlib1g:amd64\t1:1.3.dfsg+really1.3.1-1+b1\tinstall ok installed\n"
        after = "libc6:amd64\t2.41\tinstall ok installed\nzlib1g:amd64\t1:1.3.dfsg+really1.3.1-1+deb13u1\tinstall ok installed\n"
        check_zlib_layer(before, after)
        for bad in (
            before,
            after.replace("2.41", "2.42"),
            after.replace("install ok installed", "install ok half-installed", 1),
            after + "other\t1\tinstall ok installed\n",
        ):
            with self.assertRaises(ValueError):
                check_zlib_layer(before, bad)

    def test_loaded_library_controls(self):
        good = "PROCESS postgres\n/lib/libicuuc.so.76\n/lib/libssl.so.3\n/lib/libcrypto.so.3\n/lib/libz.so.1\nPROCESS psql\n/lib/libreadline.so.8\n/lib/libtinfo.so.6\n"
        check_loaded(good)
        for bad in ("", good.replace("libreadline.so.8", "missing"), good + "/lib/libldap.so.2\n"):
            with self.assertRaises(ValueError):
                check_loaded(bad)

    def test_file_tree_controls(self):
        files = (
            "a" * 64 + "  /opt/pg-smoke/bin/postgres\n" + "b" * 64 + "  /opt/pg-smoke/bin/psql\n"
        )
        metadata = "755 0 0 /opt/pg-smoke \n755 0 0 /opt/pg-smoke/bin/postgres \n"
        check_tree(files, files, metadata, metadata)
        for values in (
            ("", "", metadata, metadata),
            (files, files + "changed", metadata, metadata),
            (files, files, metadata, metadata + "changed"),
            ("fake", "fake", metadata, metadata),
            (
                files,
                files,
                "755 1000 0 /opt/pg-smoke/bin/postgres",
                "755 1000 0 /opt/pg-smoke/bin/postgres",
            ),
        ):
            with self.assertRaises(ValueError):
                check_tree(*values)

    def test_linkage_controls(self):
        good = (
            "FILE /opt/pg-smoke/bin/postgres\nlibicuuc.so.76 libssl.so.3 libcrypto.so.3 libz.so.1\n"
            "FILE /opt/pg-smoke/bin/psql\nlibreadline.so.8 libtinfo.so.6\n"
            "FILE /opt/pg-smoke/lib/postgresql/plpgsql.so\nstatically linked\n"
            "FILE /opt/pg-smoke/lib/libpq.so.5.18\nlibssl.so.3\n"
        )
        check_linkage(good)
        for bad in (
            "",
            good + "libldap.so.2 => /lib/libldap.so.2\n",
            good + "libcurl.so.4 => /lib/libcurl.so.4\n",
            good + "libmissing => not found\n",
            good.replace("libreadline.so.8", "missing"),
            good.replace("plpgsql.so", "other.so"),
        ):
            with self.assertRaises(ValueError):
                check_linkage(bad)


if __name__ == "__main__":
    unittest.main()
