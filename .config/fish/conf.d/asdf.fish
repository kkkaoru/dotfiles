# asdf shims (asdf は ~/.local/bin/asdf に公式バイナリを配置)
# terraform は ~/.local/bin/terraform ラッパー (リポジトリの .versenv.env に追従) を優先させるため末尾に追加
fish_add_path --global --move --append $HOME/.asdf/shims
