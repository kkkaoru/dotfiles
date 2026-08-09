# Command Code (`cmd`) must not pick up Homebrew Node 26 or bun's global bin.
function __command_code_wrapper
    set -l wrapper $HOME/.local/bin/cmd
    if not test -x $wrapper
        set -l here (status filename)
        if test -L $here
            set here (realpath $here)
        end
        set wrapper (path resolve (dirname $here)/../../../scripts/command-code-cmd)
    end
    if not test -x $wrapper
        echo "cmd: wrapper がありません。dotfiles で ./create-symlinks.sh を実行してください。" >&2
        return 1
    end
    $wrapper $argv
end

function cmd --description 'Command Code via mise Node LTS'
    __command_code_wrapper $argv
end

function cmdc --description 'Command Code via mise Node LTS'
    __command_code_wrapper $argv
end

function command-code --description 'Command Code via mise Node LTS'
    __command_code_wrapper $argv
end

function commandcode --description 'Command Code via mise Node LTS'
    __command_code_wrapper $argv
end
