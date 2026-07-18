function herdr-cwd --description "Open a Herdr workspace for the current directory path"
    argparse -i 'd/directory-session' -- $argv; or return 2

    set -l real_pwd (pwd -P)
    set -l dir_name (basename "$real_pwd")
    set -l safe_dir (string replace -ra '[^A-Za-z0-9._-]+' '_' -- "$dir_name" | string sub -l 10)
    set -l path_hash (printf '%s' "$real_pwd" | cksum | string split ' ' | head -n 1 | string sub -l 8)
    set -l workspace_label "$safe_dir-$path_hash"

    set -q _flag_directory_session; and set -p argv --session "$workspace_label"

    # Keep Herdr's normal CLI behavior when options or subcommands are supplied.
    if test (count $argv) -gt 0
        command herdr $argv
        return $status
    end

    set -l workspace_json (command herdr workspace list 2>/dev/null)
    if test $status -ne 0
        # The first launch creates its initial workspace in the current directory.
        command herdr
        set -l herdr_status $status

        # Give that initial workspace the same stable path-based label, if the
        # persistent server is still available after detaching.
        set -l snapshot_json (command herdr api snapshot 2>/dev/null)
        if test $status -eq 0
            set -l initial_workspace_id (printf '%s\n' "$snapshot_json" | command jq -r --arg cwd "$real_pwd" '.result.snapshot.panes[] | select(.cwd == $cwd or .foreground_cwd == $cwd) | .workspace_id' | head -n 1)
            if test -n "$initial_workspace_id"
                command herdr workspace rename "$initial_workspace_id" "$workspace_label" >/dev/null 2>&1
            end
        end

        return $herdr_status
    end

    set -l workspace_id (printf '%s\n' "$workspace_json" | command jq -r --arg label "$workspace_label" '.result.workspaces[] | select(.label == $label) | .workspace_id' | head -n 1)

    if test -n "$workspace_id"
        command herdr workspace focus "$workspace_id" >/dev/null
    else
        command herdr workspace create --cwd "$real_pwd" --label "$workspace_label" --focus >/dev/null
    end

    if test $status -ne 0
        echo "hdr: failed to prepare Herdr workspace '$workspace_label'" >&2
        return 1
    end

    command herdr
end
