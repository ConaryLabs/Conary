# packaging/dracut/90conary/conary-verity.sh
# Shared composefs verification policy for every Conary-owned initramfs.
# shellcheck shell=sh

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
