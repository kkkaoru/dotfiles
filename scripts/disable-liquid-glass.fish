#!/usr/bin/env fish
#
# macOS 27 の Liquid Glass 表現と窓・メニューのアニメーションを無効化する。
# キーはシステムライブラリの文字列から推測したもので、効果は未検証。
# 反映には各アプリの再起動（確実にはログアウト）が必要。
#
# 使い方:
#   fish scripts/disable-liquid-glass.fish           # 無効化。管理者パスワードは1回
#   fish scripts/disable-liquid-glass.fish restore   # 書いたキーを削除して初期値に戻す
#

if test (uname -s) != Darwin
    echo "このスクリプトは macOS 専用です" >&2
    exit 1
end

# ユーザー全体 (-g) とシステム全体（ログイン画面などほかのユーザーも含む）の両方に書く。
set -l system_domain /Library/Preferences/.GlobalPreferences
set -l bool_keys \
    NSWindowEnableGlassStyle \
    NSGlassMenusEnabled \
    NSGlassMenuLayoutEnabled \
    NSGlassSuggestionMenusEnabled \
    NSGlassSuggestionMenuWindowsEnabled \
    NSMorphingMenusEnabled \
    NSAutomaticWindowAnimationsEnabled
# shortcut: 0 が「反射なし」かは不明。効かなければ値を変えて試す。
set -l int_keys CAEdgeSpecularMode

# NSGlassDiffusionSetting はスライダー値の型・向きが不明なので触らない。

sudo -v; or exit 1

switch "$argv[1]"
    case ''
        for key in $bool_keys
            defaults write -g $key -bool false
            sudo defaults write $system_domain $key -bool false
        end
        for key in $int_keys
            defaults write -g $key -int 0
            sudo defaults write $system_domain $key -int 0
        end
        echo "無効化しました。アプリを再起動するかログアウトしてください。"
    case restore
        for key in $bool_keys $int_keys
            defaults delete -g $key 2>/dev/null
            sudo defaults delete $system_domain $key 2>/dev/null
        end
        echo "初期値に戻しました。アプリを再起動するかログアウトしてください。"
    case '*'
        echo "使い方: fish (status filename) [restore]" >&2
        exit 2
end
