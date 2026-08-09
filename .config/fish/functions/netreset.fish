function netreset --description 'Resync Mac network state without route/service deletion'
    set -l tool "$HOME/.local/bin/mac-wifi-stability"
    if not test -x "$tool"
        echo "netreset: install mac-wifi-stability first" >&2
        return 1
    end
    if test (count $argv) -gt 1
        echo "netreset: accepts at most one option" >&2
        return 2
    end
    switch "$argv[1]"
        case --soft
            command "$tool" --soft
        case --force --rebind
            command "$tool" --force-rebind
        case --status
            command "$tool" --status
        case --ohomemesh --connect-ohomemesh
            command "$tool" --ohomemesh
        case --all-down --nuclear --soft-full --logout --userspace
            echo "netreset: $argv[1] is intentionally disabled; use --soft (no logout/reboot)" >&2
            return 2
        case ''
            command "$tool" --force-rebind
        case '*'
            echo "netreset: unknown option: $argv[1]" >&2
            return 2
    end
end
