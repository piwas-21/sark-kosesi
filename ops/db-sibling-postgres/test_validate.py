#!/usr/bin/env python3
"""Negative controls for this experiment's measured closure."""

import unittest

from validate import check_image, check_linkage, check_loaded, check_packages, check_tree


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
        sibling = {"libtinfo6": "6.5+20250216-2", "libicu76": "76.1-4"}
        candidate = {**sibling, "libreadline8t64": "8.2-6", "readline-common": "8.2-6"}
        check_packages(sibling, candidate)
        for bad in (
            sibling,
            {**candidate, "libldap2": "1"},
            {**candidate, "libreadline8t64": "wrong"},
        ):
            with self.assertRaises(ValueError):
                check_packages(sibling, bad)

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
