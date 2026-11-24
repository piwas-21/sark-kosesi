#!/bin/sh
set -eu

# The donor is the digest-pinned Distroless CC image for this build's platform.
# Its Debian manifests list regular files. The package layers also contain the
# five symlinks copied below, which Debian does not list in the checksums.
output=$2
mkdir "$output"
cd "$1"

copy_entry() {
    mkdir -p "$output/${1%/*}"
    cp -Pp "$1" "$output/$1"
}

for package in gcc-14-base libgcc-s1 libstdc++6 libgomp1; do
    status="var/lib/dpkg/status.d/$package"
    checksums="$status.md5sums"
    test -s "$checksums"
    while read -r checksum entry; do
        case "$entry/" in
            usr/lib/*|usr/share/*) ;;
            *) exit 1 ;;
        esac
        case "/$entry/" in
            *'/../'*|*'/./'*|*'//'*) exit 1 ;;
        esac
        test "${#checksum}" -eq 32
        test -f "$entry"
        test ! -L "$entry"
        copy_entry "$entry"
    done < "$checksums"
    md5sum -c "$checksums" >/dev/null
    copy_entry "$status"
    copy_entry "$checksums"
done

set -- usr/lib/*/libstdc++.so.6 usr/lib/*/libgomp.so.1
test "$#" -eq 2
for entry in "$@" usr/share/doc/libgcc-s1 usr/share/doc/libstdc++6 usr/share/doc/libgomp1; do
    test -L "$entry"
    case "$(readlink "$entry")" in
        ''|/*|*/*|.|..) exit 1 ;;
    esac
    copy_entry "$entry"
    test -e "$output/$entry"
done
