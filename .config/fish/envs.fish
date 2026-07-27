set -x LDFLAGS -L/usr/local/opt/openssl/lib
set -x CPPFLAGS -I/usr/local/opt/openssl/include
set -x ANDROID_HOME /usr/local/share/android-sdk
set -x PATH /usr/local/opt/openssl/bin $PATH
set -x PATH $PATH:$ANDROID_HOME/tools:$ANDROID_HOME/platform-tools $PATH
set -g fish_user_paths /usr/local/sbin $fish_user_paths
set -g fish_user_paths /usr/local/opt/icu4c/bin $fish_user_paths
set -g fish_user_paths /usr/local/opt/icu4c/sbin $fish_user_paths
set -gx LDFLAGS -L/usr/local/opt/icu4c/lib
set -gx CPPFLAGS -I/usr/local/opt/icu4c/include
set -gx PKG_CONFIG_PATH /usr/local/opt/icu4c/lib/pkgconfig
set -g fish_user_paths /usr/local/opt/gnu-getopt/bin $fish_user_paths
# set -x ASDF_GOLANG_MOD_VERSION_ENABLED true
set -gx ASDF_GOLANG_MOD_VERSION_ENABLED true

# Fugu (SAKANA) API key for Codex Fugu models.
# Priority: explicit env > ~/.codex/.env > ~/.env
if not set -q SAKANA_AI_PRO_API_KEY
  for sakana_env_file in ~/.codex/.env ~/.env
    if not test -r $sakana_env_file
      continue
    end
    set -l sakana_env_line (string match -r '^SAKANA_AI_PRO_API_KEY=.*' < $sakana_env_file)
    if test -z "$sakana_env_line"
      continue
    end
    set -l sakana_api_key (string replace -r '^SAKANA_AI_PRO_API_KEY=' '' -- $sakana_env_line)
    set sakana_api_key (string trim -c "'" -- (string trim -c '"' -- $sakana_api_key))
    if test -n "$sakana_api_key"
      set -gx SAKANA_AI_PRO_API_KEY $sakana_api_key
      break
    end
  end
end
