
_conary_bridge_release () {
    printf '\n' >&9
}

_conary_bridge_send_value () {
    local _conary_bridge_value="$1"
    local LC_ALL=C
    local _conary_bridge_length="${#_conary_bridge_value}"
    printf '%s\n' "$_conary_bridge_length" >&7 &&
        printf '%s' "$_conary_bridge_value" >&7
}

_conary_bridge_exchange () {
    local _conary_bridge_identity="$1"
    shift
    local _conary_bridge_argc="$#"
    local _conary_bridge_argument
    local _conary_bridge_magic
    local _conary_bridge_version
    local _conary_bridge_status
    local _conary_bridge_stdout_length
    local _conary_bridge_stderr_length
    local _conary_bridge_extra
    local _conary_bridge_stdout
    local _conary_bridge_stderr
    local LC_ALL=C

    if ! IFS= read -r _conary_bridge_token <&8; then
        printf '%s\n' 'conary lifecycle bridge lock channel closed' >&2
        return 125
    fi
    if ! printf 'CONARY-LIFECYCLE-BRIDGE 1 %s\n' "$_conary_bridge_argc" >&7 ||
       ! _conary_bridge_send_value "$_conary_bridge_identity"; then
        _conary_bridge_release
        printf '%s\n' 'conary lifecycle bridge request channel closed' >&2
        return 125
    fi
    for _conary_bridge_argument in "$@"; do
        if ! _conary_bridge_send_value "$_conary_bridge_argument"; then
            _conary_bridge_release
            printf '%s\n' 'conary lifecycle bridge request channel closed' >&2
            return 125
        fi
    done

    if ! IFS=' ' read -r _conary_bridge_magic _conary_bridge_version \
        _conary_bridge_status _conary_bridge_stdout_length \
        _conary_bridge_stderr_length _conary_bridge_extra <&7 ||
       [ "$_conary_bridge_magic" != 'CONARY-LIFECYCLE-BRIDGE' ] ||
       [ "$_conary_bridge_version" != '1' ] ||
       [ -n "$_conary_bridge_extra" ]; then
        _conary_bridge_release
        printf '%s\n' 'conary lifecycle bridge response header is malformed' >&2
        return 125
    fi
    case "$_conary_bridge_status:$_conary_bridge_stdout_length:$_conary_bridge_stderr_length" in
        *[!0-9:]*|:*|*::*|*:)
            _conary_bridge_release
            printf '%s\n' 'conary lifecycle bridge response lengths are malformed' >&2
            return 125
            ;;
    esac
    if [ "$_conary_bridge_status" -gt 255 ]; then
        _conary_bridge_release
        printf '%s\n' 'conary lifecycle bridge response status is out of range' >&2
        return 125
    fi
    if ! IFS= read -r _conary_bridge_stdout <&7 ||
       ! IFS= read -r _conary_bridge_stderr <&7 ||
       [ "${#_conary_bridge_stdout}" -ne "$_conary_bridge_stdout_length" ] ||
       [ "${#_conary_bridge_stderr}" -ne "$_conary_bridge_stderr_length" ]; then
        _conary_bridge_release
        printf '%s\n' 'conary lifecycle bridge response body is malformed' >&2
        return 125
    fi
    _conary_bridge_release
    _CONARY_BRIDGE_STDOUT="$_conary_bridge_stdout"
    _CONARY_BRIDGE_STDERR="$_conary_bridge_stderr"
    _CONARY_BRIDGE_STATUS="$_conary_bridge_status"
    return 0
}

_conary_bridge_emit_response () {
    printf '%b' "$_CONARY_BRIDGE_STDOUT"
    printf '%b' "$_CONARY_BRIDGE_STDERR" >&2
    return "$_CONARY_BRIDGE_STATUS"
}
