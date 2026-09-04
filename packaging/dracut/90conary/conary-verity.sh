# packaging/dracut/90conary/conary-verity.sh
# Binary-free adapter for generation::verity_policy::VerityPolicy in conary-core.
# Packaging and bootstrap initramfs images contain no conary executable. Rust
# conformance tests bind this adapter to that owner until consolidation (#863).
# shellcheck shell=sh

conary_read_verity() {
    conary_verity_cmdline_file="$1"
    conary_verity_present=0
    conary_verity_result=""

    if [ -r "$conary_verity_cmdline_file" ]; then
        # Kernel command-line arguments are whitespace-delimited. Repeated
        # arguments follow the kernel/systemd convention: the last wins.
        # shellcheck disable=SC2013
        for conary_verity_opt in $(cat "$conary_verity_cmdline_file"); do
            case "$conary_verity_opt" in
                conary.verity=*)
                    conary_verity_present=1
                    conary_verity_result="${conary_verity_opt#conary.verity=}"
                    ;;
            esac
        done
    fi

    # Only absence selects the default. A present empty value must reach the
    # invalid-value branch in conary_composefs_options.
    if [ "$conary_verity_present" -eq 0 ]; then
        conary_verity_result=on
    fi
    printf '%s\n' "$conary_verity_result"
}

conary_composefs_options() {
    conary_verity_value="$1"
    conary_verity_basedir="$2"

    case "$conary_verity_value" in
        on)
            printf 'basedir=%s,verity_check=1\n' "$conary_verity_basedir"
            ;;
        off)
            printf 'conary: WARNING: conary.verity=off disables composefs fs-verity verification\n' >&2
            printf 'basedir=%s\n' "$conary_verity_basedir"
            ;;
        *)
            printf "conary: invalid conary.verity value '%s'; expected on or off\n" \
                "$conary_verity_value" >&2
            return 1
            ;;
    esac
}
