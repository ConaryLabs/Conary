#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -eq 0 ]]; then
  echo "usage: $0 PACKAGE [PACKAGE ...]" >&2
  exit 2
fi

missing_packages=()
for package in "$@"; do
  if [[ ! "$package" =~ ^[a-z0-9][a-z0-9+.-]*$ ]]; then
    echo "invalid Ubuntu package name: $package" >&2
    exit 2
  fi

  status="$(
    dpkg-query --show --showformat='${Status}' "$package" 2>/dev/null || true
  )"
  if [[ "$status" != "install ok installed" ]]; then
    missing_packages+=("$package")
  fi
done

if [[ "${#missing_packages[@]}" -eq 0 ]]; then
  echo "Requested Ubuntu packages are already installed: $*"
  exit 0
fi

ubuntu_sources=/etc/apt/sources.list.d/ubuntu.sources
if [[ ! -f "$ubuntu_sources" || -L "$ubuntu_sources" ]]; then
  echo "canonical Ubuntu apt source is not a plain file: $ubuntu_sources" >&2
  exit 1
fi

apt_options=(
  -o "Dir::Etc::sourcelist=${ubuntu_sources}"
  -o "Dir::Etc::sourceparts=/dev/null"
)
sudo apt-get "${apt_options[@]}" update
sudo env DEBIAN_FRONTEND=noninteractive \
  apt-get "${apt_options[@]}" install -y --no-install-recommends \
  "${missing_packages[@]}"
