function zellij-cwd --description "Start a Zellij session named after the current directory path"
    set -l real_pwd (pwd -P)
    set -l dir_name (basename "$real_pwd")
    set -l safe_dir (string replace -ra '[^A-Za-z0-9._-]+' '_' -- "$dir_name" | string sub -l 24)
    set -l path_hash (printf '%s' "$real_pwd" | cksum | string split ' ' | head -n 1)
    set -l base_session "$safe_dir-$path_hash"
    set -l usage "Usage: zellij-cwd [NUMBER|new]"

    if test (count $argv) -gt 1
        echo $usage >&2
        return 2
    end

    if test (count $argv) -eq 0
        command zellij attach "$base_session" --create
        return $status
    end

    set -l arg $argv[1]
    switch $arg
        case -h --help
            echo $usage
            echo "  no argument: attach/create a session named with the current directory name and path hash"
            echo "  NUMBER:      attach/create a session named current-directory-name-path-hash-NUMBER"
            echo "  new:         create a new session using the next available numeric suffix"
            return 0
        case new --new -n
            set -l existing_sessions (command zellij list-sessions --short --no-formatting 2>/dev/null)
            set -l suffix 1
            set -l session_name "$base_session-$suffix"

            while contains -- "$session_name" $existing_sessions
                set suffix (math $suffix + 1)
                set session_name "$base_session-$suffix"
            end

            command zellij --session "$session_name"
        case '*'
            if not string match -qr '^[0-9]+$' -- "$arg"
                echo $usage >&2
                return 2
            end

            command zellij attach "$base_session-$arg" --create
    end
end
