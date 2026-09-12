function __recover_working_directory --description 'Recover a deleted working directory before launching a CLI'
    # Fish's builtin pwd can return a cached path even after its directory is deleted.
    if command /bin/pwd -P >/dev/null 2>&1
        return 0
    end

    if not builtin cd -- "$PWD" 2>/dev/null
        builtin cd -- "$HOME"
        or return 1
    end
    printf 'Recovered unavailable working directory; using %s\n' "$PWD" >&2
end
