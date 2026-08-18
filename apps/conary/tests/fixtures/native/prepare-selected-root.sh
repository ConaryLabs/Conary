#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <db-path> <selected-runtime-root>" >&2
  exit 64
fi

db_path="$1"
runtime_root="$2"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixtures_dir="$(cd "${script_dir}/.." && pwd)"
conary_bin="${CONARY_BIN:-conary}"
layout_fixture="${fixtures_dir}/native-selected-root-layout"
authority_dir="${fixtures_dir}/ccs-test-authority"
work="/tmp/conary-selected-root-layout"
stage="${work}/stage"
output="${work}/output"

case "${db_path}" in
  /*) ;;
  *)
    echo "Database path must be absolute: ${db_path}" >&2
    exit 64
    ;;
esac
case "${runtime_root}" in
  /)
    echo "The host root is not a selected-generation execution target" >&2
    exit 64
    ;;
  /*) ;;
  *)
    echo "Selected runtime root must be absolute: ${runtime_root}" >&2
    exit 64
    ;;
esac
if [[ ! -f "${db_path}" ]]; then
  echo "Conary database does not exist: ${db_path}" >&2
  exit 1
fi

rm -rf "${work}"
mkdir -p "${stage}" "${output}"
cp -a "${layout_fixture}/stage/." "${stage}/"
chmod 0600 "${stage}/etc/shadow" "${stage}/etc/gshadow"

copy_runtime_path() {
  local source="$1"
  if [[ "${source}" != /* || ! -e "${source}" ]]; then
    echo "Selected-root runtime input is not an existing absolute path: ${source}" >&2
    exit 1
  fi
  cp --dereference --preserve=mode,timestamps --parents "${source}" "${stage}"
}

copy_elf_closure() {
  local executable="$1"
  local ldd_output
  local dependency
  copy_runtime_path "${executable}"
  if ! ldd_output="$(LC_ALL=C ldd "${executable}" 2>&1)"; then
    if [[ "${ldd_output}" == *"not a dynamic executable"* ]]; then
      return
    fi
    echo "Unable to resolve runtime closure for ${executable}: ${ldd_output}" >&2
    exit 1
  fi
  if [[ "${ldd_output}" == *"not found"* ]]; then
    echo "Runtime closure for ${executable} contains an unresolved library:" >&2
    echo "${ldd_output}" >&2
    exit 1
  fi
  while IFS= read -r dependency; do
    [[ -n "${dependency}" ]] || continue
    copy_runtime_path "${dependency}"
  done < <(
    awk '
      $2 == "=>" && $3 ~ /^\// {
        if ($1 ~ /^\//) { print $1 }
        print $3
        next
      }
      $1 ~ /^\// { print $1 }
    ' <<<"${ldd_output}" | sort -u
  )
}

# The selected-root layout fixture declares x86_64 + GNU libc. The kernel opens
# PT_INTERP by its encoded absolute path, while ldd can report that path on the
# left of `=>` and its canonical loader target on the right on merged-/usr
# distributions such as Arch. copy_elf_closure materializes both paths so the
# selected generation is executable without inheriting host symlink topology.
gnu_loader=/lib64/ld-linux-x86-64.so.2
copy_runtime_path "${gnu_loader}"
copy_elf_closure /bin/sh
copy_elf_closure /bin/bash
copy_elf_closure /usr/bin/bash

install_path="$(command -v install)"
for command_name in mkdir getent groupadd useradd; do
  command_path="$(command -v "${command_name}")"
  copy_elf_closure "${command_path}"
done
copy_elf_closure "${install_path}"
copy_elf_closure /bin/false
copy_elf_closure /usr/bin/true

# Information-only boot-runtime invocations execute the provider staged from
# the selected root. The daily-driver lifecycle calls the pinned absolute
# depmod endpoint, so carry its executable and ELF closure into that root;
# mutation forms remain intercepted by the typed boot-runtime boundary.
copy_elf_closure /usr/sbin/depmod

# A selected root advertises service-manager lifecycle support only when its
# provider executable is part of that root. The deterministic fixture is the
# execution boundary, while the booted host's real systemctl remains the
# persisted capability authority. Runtime verbs are bind-intercepted before
# the fixture body can execute, avoiding target-specific dlopen dependencies.
if command -v systemctl >/dev/null 2>&1; then
  systemctl_path=/usr/bin/systemctl
  if [[ ! -x "${stage}${systemctl_path}" ]]; then
    echo "Selected-root systemctl fixture is missing: ${stage}${systemctl_path}" >&2
    exit 1
  fi
else
  systemctl_path=
  rm -f "${stage}/usr/bin/systemctl"
fi
if openrc_path="$(command -v rc-service 2>/dev/null)"; then
  copy_elf_closure "${openrc_path}"
fi

for config_path in \
  /etc/default/useradd \
  /etc/login.defs \
  /etc/nsswitch.conf
do
  if [[ -f "${config_path}" ]]; then
    copy_runtime_path "${config_path}"
  fi
done

while IFS= read -r nss_library; do
  copy_runtime_path "${nss_library}"
done < <(
  find /lib /lib64 /usr/lib /usr/lib64 \
    -type f \
    \( -name 'libnss_files.so.2' -o -name 'libnss_compat.so.2' \) \
    -print 2>/dev/null | sort -u
)

mkdir -p "${stage}/dev" "${runtime_root}/boot/EFI/BOOT"
: > "${stage}/dev/null"
printf 'selected-root-test-kernel\n' > "${runtime_root}/boot/vmlinuz-test-kernel"
printf 'selected-root-test-initramfs\n' > "${runtime_root}/boot/initramfs-test-kernel.img"
printf 'selected-root-test-efi\n' > "${runtime_root}/boot/EFI/BOOT/BOOTX64.EFI"

"${conary_bin}" ccs build "${layout_fixture}" \
  --source "${stage}" \
  --output "${output}" \
  --target ccs \
  --key "${authority_dir}/fixture-signing-key.private"

mapfile -t layout_packages < <(
  find "${output}" -maxdepth 1 -type f -name '*.ccs' -print | sort
)
if [[ "${#layout_packages[@]}" -ne 1 ]]; then
  echo "Expected one selected-root layout package, found ${#layout_packages[@]}" >&2
  exit 1
fi

CONARY_TEST_SKIP_GENERATION_MOUNT=1 \
  "${conary_bin}" ccs install "${layout_packages[0]}" \
    --policy "${authority_dir}/trust-policy.toml" \
    --db-path "${db_path}" \
    --root "${runtime_root}" \
    --sandbox always \
    --no-deps \
    --yes

"${script_dir}/assert-selected-generation.py" \
  --root "${runtime_root}" \
  --present /bin/sh \
  --present /bin/bash \
  --present /bin/false \
  --present /usr/bin/bash \
  --present /usr/sbin/depmod \
  --present "${systemctl_path:-/usr/bin/true}" \
  --present "${install_path}" \
  --present /lib64/ld-linux-x86-64.so.2 \
  --present /etc/passwd \
  --present /etc/group \
  --present /sbin/init
