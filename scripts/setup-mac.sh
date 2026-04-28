#!/bin/bash
#
# setup-mac.sh - macOS のシステム設定をまとめて適用するスクリプト
#
# 使い方:
#   新しい Mac で以下のコマンドを実行する:
#
#     curl -fsSL https://raw.githubusercontent.com/kkkaoru/dotfiles/master/scripts/setup-mac.sh | bash
#
#   または、リポジトリを clone 済みなら:
#
#     bash scripts/setup-mac.sh
#
#   sudo が必要な項目があるためパスワードを求められる場合があります。
#   設定変更を反映するため、最後に Dock / Finder / SystemUIServer / cfprefsd を再起動します。
#

set -euo pipefail

echo "==> macOS のシステム設定を適用します"

# sudo を最初に通しておき、以後はバックグラウンドで延命する
sudo -v
while true; do sudo -n true; sleep 60; kill -0 "$$" || exit; done 2>/dev/null &

# ---------------------------------------------------------------------------
# 一般: 外観モードをダークに変更
# ---------------------------------------------------------------------------
echo "--> 外観モードを Dark に変更"
defaults write -g AppleInterfaceStyle -string "Dark"

# ---------------------------------------------------------------------------
# マウス: スクロール方向のナチュラルを OFF
# ---------------------------------------------------------------------------
echo "--> マウスの『スクロール方向: ナチュラル』をオフ"
defaults write -g com.apple.swipescrolldirection -bool false

# ---------------------------------------------------------------------------
# キーボード ショートカット
#   - 入力ソース > 前の入力ソースを選択: Cmd + Space   (id: 60)
#   - Spotlight > Spotlight 検索を表示: Ctrl + Space   (id: 64)
#
# parameters の意味: (ASCII, keyCode, modifierMask)
#   space ASCII = 32, keyCode = 49
#   modifier mask: Command = 1048576, Control = 262144
# ---------------------------------------------------------------------------
echo "--> ショートカット: 前の入力ソース = Cmd+Space, Spotlight = Ctrl+Space"
defaults write com.apple.symbolichotkeys AppleSymbolicHotKeys -dict-add 60 \
  '{ enabled = 1; value = { parameters = (32, 49, 1048576); type = "standard"; }; }'
defaults write com.apple.symbolichotkeys AppleSymbolicHotKeys -dict-add 64 \
  '{ enabled = 1; value = { parameters = (32, 49, 262144); type = "standard"; }; }'

# ---------------------------------------------------------------------------
# Bluetooth: メニューバーに表示
# ---------------------------------------------------------------------------
echo "--> メニューバーに Bluetooth を表示"
defaults write com.apple.controlcenter "NSStatusItem Visible Bluetooth" -bool true
defaults -currentHost write com.apple.controlcenter Bluetooth -int 18

# ---------------------------------------------------------------------------
# サウンド: メニューバーに音量を表示
# ---------------------------------------------------------------------------
echo "--> メニューバーに音量を表示"
defaults write com.apple.controlcenter "NSStatusItem Visible Sound" -bool true
defaults -currentHost write com.apple.controlcenter Sound -int 18

# ---------------------------------------------------------------------------
# 共有: コンピューター名 / LocalHostName を kkk4oru に変更
# ---------------------------------------------------------------------------
NEW_HOSTNAME="kkk4oru"
echo "--> コンピューター名を ${NEW_HOSTNAME} に変更"
sudo scutil --set ComputerName "${NEW_HOSTNAME}"
sudo scutil --set HostName "${NEW_HOSTNAME}"
sudo scutil --set LocalHostName "${NEW_HOSTNAME}"
sudo defaults write /Library/Preferences/SystemConfiguration/com.apple.smb.server \
  NetBIOSName -string "${NEW_HOSTNAME}"

# ---------------------------------------------------------------------------
# デスクトップとスクリーンセーバー
#   - 左下ホットコーナー: ディスプレイをスリープ (action = 10)
#   - スクリーンセーバーを開始しない (idleTime = 0)
#   - デスクトップ画像はスキップ (手動で設定)
# ---------------------------------------------------------------------------
echo "--> 左下ホットコーナーに『ディスプレイをスリープ』を割り当て"
defaults write com.apple.dock wvous-bl-corner -int 10
defaults write com.apple.dock wvous-bl-modifier -int 0

echo "--> スクリーンセーバーを開始しないに変更"
defaults -currentHost write com.apple.screensaver idleTime -int 0

# ---------------------------------------------------------------------------
# セキュリティとプライバシー: スリープ/スクリーンセーバー解除に
#   開始 5 秒後からパスワードを要求
# ---------------------------------------------------------------------------
echo "--> パスワード要求を『開始 5 秒後』に変更"
defaults write com.apple.screensaver askForPassword -int 1
defaults write com.apple.screensaver askForPasswordDelay -int 5

# ---------------------------------------------------------------------------
# アクセシビリティ: カーソルサイズを中間 (2.5) に
#   1.0 = 通常, 4.0 = 最大
# ---------------------------------------------------------------------------
echo "--> カーソルサイズを 2.5 に変更"
defaults write com.apple.universalaccess mouseDriverCursorSize -float 2.5

# ---------------------------------------------------------------------------
# Finder: すべてのファイル名拡張子を表示
# ---------------------------------------------------------------------------
echo "--> Finder: すべてのファイル名拡張子を表示"
defaults write NSGlobalDomain AppleShowAllExtensions -bool true

# ---------------------------------------------------------------------------
# Dock
#   - 自動的に非表示
#   - サイズを最大 (128)
#   - 拡大を OFF
# ---------------------------------------------------------------------------
echo "--> Dock を自動的に非表示 / サイズ最大 / 拡大オフ"
defaults write com.apple.dock autohide -bool true
defaults write com.apple.dock tilesize -int 128
defaults write com.apple.dock magnification -bool false

# ---------------------------------------------------------------------------
# バッテリー: ディスプレイをオフにするまでの時間 → しない
#   -b: バッテリー駆動時, -c: 電源アダプタ接続時
# ---------------------------------------------------------------------------
echo "--> ディスプレイをオフにする時間を『しない』に変更"
sudo pmset -b displaysleep 0
sudo pmset -c displaysleep 0

# ---------------------------------------------------------------------------
# 設定を反映するため関連プロセスを再起動
# ---------------------------------------------------------------------------
echo "==> 設定を反映するため関連プロセスを再起動"
killall Dock 2>/dev/null || true
killall Finder 2>/dev/null || true
killall SystemUIServer 2>/dev/null || true
killall cfprefsd 2>/dev/null || true

echo
echo "==> 完了しました"
echo "    一部の変更 (外観モード / ショートカット等) は再ログインまたは再起動で完全に反映されます"
