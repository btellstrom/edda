#!/usr/bin/env sh
# Downloads the SRD monsters and converts them into data/monsters/, ready
# for `monster-service import` (compose's seed job loads them by itself).
#
# Wizards of the Coast publishes the SRD (5.1 for the 2014 rules, 5.2 for
# 2024's) only as PDFs, under CC BY 4.0. The structured version used here is
# the community-maintained 5e-database (https://github.com/5e-bits/5e-database),
# pinned to one commit and checked against known checksums, so everyone gets
# the same monsters. Needs curl, sha256sum and python3.
#
#   tools/fetch-srd-monsters.sh [OUTPUT DIR]   (default: data/monsters)
set -eu

COMMIT=a6212beb4b278917c2cff41c2b04c4ef4f3e0d6f
BASE="https://raw.githubusercontent.com/5e-bits/5e-database/$COMMIT/src"
SHA_2014=cbf1b6d0a94f6e13afd1480d647d288c61ba0ddc3d3a7842d1501852dc954237
SHA_2024=34ec39f2b384a5e1ed11ac6d9e17601a19579ed67fd1775d21588d186714346f

here=$(cd "$(dirname "$0")" && pwd)
out=${1:-"$here/../data/monsters"}
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() { # edition expected-sha256 srd-version
    file="$work/$1.json"
    echo "Downloading the $1 monsters (SRD $3)..."
    curl --fail --silent --show-error --location -o "$file" "$BASE/$1/en/5e-SRD-Monsters.json"
    actual=$(sha256sum "$file" | cut -d' ' -f1)
    if [ "$actual" != "$2" ]; then
        echo "error: the $1 file's checksum is $actual, expected $2" >&2
        exit 1
    fi
    python3 -I "$here/convert-5e-database.py" --edition "$1" "$file" > "$out/srd-$3.json"
    echo "  wrote $out/srd-$3.json"
}

fetch 2014 "$SHA_2014" 5.1
fetch 2024 "$SHA_2024" 5.2
echo "Attribution for this material: data/monsters/README.md"
