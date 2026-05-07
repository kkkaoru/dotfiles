if test -d ~/.cargo
    fish_add_path $HOME/.cargo/bin
end

# Add dotfiles scripts to PATH
if test -d ~/dotfiles/scripts
    fish_add_path $HOME/dotfiles/scripts
end

if test -d $HOME/.bun/bin
    fish_add_path $HOME/.bun/bin
end
