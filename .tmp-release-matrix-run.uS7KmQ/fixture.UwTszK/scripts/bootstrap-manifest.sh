#!/usr/bin/env bash
# scripts/bootstrap-manifest.sh -- Build the signed-input manifest for release bootstrap.
set -euo pipefail

die() {
    printf 'bootstrap manifest: %s\n' "$1" >&2
    exit 1
}

if [[ $# -ne 4 ]]; then
    die "usage: $0 <tag> <version> <release-directory> <output>"
fi

tag="$1"
version="$2"
release_directory="$3"
output="$4"

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "invalid suite version: $version"
[[ "$tag" == "v${version}" ]] || die "tag $tag does not bind suite version $version"
[[ -d "$release_directory" && ! -L "$release_directory" ]] ||
    die "release directory must be a real directory"
[[ ! -e "$output" && ! -L "$output" ]] || die "refusing to replace output: $output"

artifact_row() {
    local host_id="$1"
    local host_version="$2"
    local format="$3"
    local basename="$4"
    local path="${release_directory}/${basename}"
    local size digest

    [[ -f "$path" && ! -L "$path" && -s "$path" ]] ||
        die "required ${format} artifact is missing, empty, or symlinked: $path"
    size="$(stat -c '%s' "$path")"
    digest="$(sha256sum "$path" | awk '{print $1}')"
    [[ "$size" =~ ^[1-9][0-9]*$ ]] || die "invalid size for $basename"
    [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "invalid SHA-256 for $basename"

    printf 'artifact=%s|%s|x86_64|%s|%s|%s|%s\n' \
        "$host_id" "$host_version" "$format" "$basename" "$size" "$digest"
}

umask 022
{
    printf 'schema=conary-bootstrap-v1\n'
    printf 'tag_name=%s\n' "$tag"
    printf 'suite_version=%s\n' "$version"
    artifact_row \
        fedora 44 rpm \
        "conary-${version}-1.fc44.x86_64.rpm"
    artifact_row \
        ubuntu 26.04 deb \
        "conary_${version}-1_amd64.deb"
    artifact_row \
        arch rolling arch \
        "conary-${version}-1-x86_64.pkg.tar.zst"
} > "$output"
