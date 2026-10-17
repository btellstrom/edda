#!/usr/bin/env sh
# Downloads the SRD's monsters, spells and conditions and converts them into
# data/monsters/, data/spells/ and data/conditions/, ready for
# `monster-service import` (compose's seed job loads them by itself).
#
# Wizards of the Coast publishes the SRD (5.1 for the 2014 rules, 5.2 for
# 2024's) only as PDFs, under CC BY 4.0. The structured version used here is
# the community-maintained 5e-database (https://github.com/5e-bits/5e-database),
# pinned to one commit and checked against known checksums, so everyone gets
# the same text. Needs curl, sha256sum and python3.
#
#   tools/fetch-srd.sh [DATA DIR]   (default: data)
set -eu

COMMIT=a6212beb4b278917c2cff41c2b04c4ef4f3e0d6f
BASE="https://raw.githubusercontent.com/5e-bits/5e-database/$COMMIT/src"

here=$(cd "$(dirname "$0")" && pwd)
data=${1:-"$here/../data"}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fetch() { # kind file edition expected-sha256 srd-version
    file="$work/$1-$3.json"
    echo "Downloading the $3 $1 (SRD $5)..."
    curl --fail --silent --show-error --location -o "$file" "$BASE/$3/en/5e-SRD-$2.json"
    actual=$(sha256sum "$file" | cut -d' ' -f1)
    if [ "$actual" != "$4" ]; then
        echo "error: the $3 $1 file's checksum is $actual, expected $4" >&2
        exit 1
    fi
    mkdir -p "$data/$1"
    python3 -I "$here/convert-5e-database.py" --kind "$1" --edition "$3" "$file" \
        > "$data/$1/srd-$5.json"
    echo "  wrote $data/$1/srd-$5.json"
}

fetch monsters Monsters 2014 cbf1b6d0a94f6e13afd1480d647d288c61ba0ddc3d3a7842d1501852dc954237 5.1
fetch monsters Monsters 2024 34ec39f2b384a5e1ed11ac6d9e17601a19579ed67fd1775d21588d186714346f 5.2
fetch spells Spells 2014 abebe7d860ec32985b804e087ecd3986145b1b772fa0c2e43cce9964b382f2ae 5.1
fetch spells Spells 2024 16aa8e47a84c6b21ccc101c1ee7ab5b8a026da940985bc826b6b58aa1fe0b6bc 5.2
fetch conditions Conditions 2014 e2c8d211a4f72722c3490217aea948e78c5235c2d51d3c08a4fd3e9d3cd4468f 5.1
fetch conditions Conditions 2024 8c1d49897efa0f8cfd366f36f8cee62ed10b1bff051d3c6ce5827dfbb02a737a 5.2
echo "Attribution for this material: data/monsters/README.md (and beside the spells and conditions)"
