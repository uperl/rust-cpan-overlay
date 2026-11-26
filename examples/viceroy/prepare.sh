#!/bin/sh
# Build the example artifacts and the Viceroy Config Store / KV fixtures.
#
#   examples/viceroy/prepare.sh && fastly compute serve
#
# Produces (git-ignored):
#   examples/viceroy/artifacts/*.tar.gz   example distributions
#   examples/viceroy/config-store.json    Config Store contents
#   examples/viceroy/kv-store.json        KV store contents
set -eu

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$here/artifacts"

pack() { # pack <dir-name>: deterministic tarball of $work/<dir-name>
  tar -C "$work" --sort=name --mtime='2026-11-26 00:00:00Z' --owner=0 --group=0 --numeric-owner \
    -cf - "$1" | gzip -n -9 > "$here/artifacts/$1.tar.gz"
}

# Personal distribution, built from the template in src/.
cp -R "$here/src/Personal-Tools-0.01" "$work/"
pack Personal-Tools-0.01

# Patched distribution: upstream File-Which 1.27 with a version bump.
# 1.2701, not 1.27.1: under Perl version rules 1.27.1 sorts *below* 1.27.
curl -fsSL https://cpan.metacpan.org/authors/id/P/PL/PLICEASE/File-Which-1.27.tar.gz | tar -C "$work" -xzf -
mv "$work/File-Which-1.27" "$work/File-Which-1.2701"
(
  cd "$work/File-Which-1.2701"
  sed -i.bak "s/'1\.27'/'1.2701'/; s/^version 1\.27\$/version 1.2701/" lib/File/Which.pm
  sed -i.bak 's/"1\.27"/"1.2701"/' Makefile.PL META.json
  sed -i.bak "s/^version: '1\.27'/version: '1.2701'/" META.yml
  { sed -n 1,2p Changes; printf '1.2701    2026-11-26\n  - CPAN Overlay example patch (version bump only)\n\n'; sed '1,2d' Changes; } > Changes.new
  mv Changes.new Changes
  rm -f lib/File/Which.pm.bak Makefile.PL.bak META.json.bak META.yml.bak
)
pack File-Which-1.2701

# Fill digests into the manifest and publish it as generation "example-1".
manifest=$(jq -c \
  --arg pt_sha "$(sha256sum "$here/artifacts/Personal-Tools-0.01.tar.gz" | cut -d' ' -f1)" \
  --arg pt_md5 "$(md5sum "$here/artifacts/Personal-Tools-0.01.tar.gz" | cut -d' ' -f1)" \
  --argjson pt_size "$(wc -c < "$here/artifacts/Personal-Tools-0.01.tar.gz")" \
  --arg fw_sha "$(sha256sum "$here/artifacts/File-Which-1.2701.tar.gz" | cut -d' ' -f1)" \
  --arg fw_md5 "$(md5sum "$here/artifacts/File-Which-1.2701.tar.gz" | cut -d' ' -f1)" \
  --argjson fw_size "$(wc -c < "$here/artifacts/File-Which-1.2701.tar.gz")" \
  '(.distributions[] | select(.name == "Personal-Tools")) |= (.sha256 = $pt_sha | .md5 = $pt_md5 | .size = $pt_size)
   | (.distributions[] | select(.name == "File-Which")) |= (.sha256 = $fw_sha | .md5 = $fw_md5 | .size = $fw_size)' \
  "$here/manifest.json")
jq -n --arg m "$manifest" '{active: "example-1", "manifest_example-1": $m}' > "$here/config-store.json"

jq -n '{
  "Personal-Tools-0.01.tar.gz": {file: "examples/viceroy/artifacts/Personal-Tools-0.01.tar.gz"},
  "File-Which-1.2701.tar.gz": {file: "examples/viceroy/artifacts/File-Which-1.2701.tar.gz"}
}' > "$here/kv-store.json"

echo "prepared: $(ls "$here/artifacts")"
