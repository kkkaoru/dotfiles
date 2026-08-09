function netdiag --description 'Show Mac Wi-Fi and gateway health'
    set -l tool "$HOME/.local/bin/mac-wifi-stability"
    if not test -x "$tool"
        echo "netdiag: install mac-wifi-stability first" >&2
        return 1
    end
    command "$tool" --status
end
