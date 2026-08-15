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

# Home-scoped secrets for cwd-independent launches.
# Priority: explicit env > ~/.codex/.env > ~/.env
function __claudex_export_home_env --argument-names env_name
  if set -q $env_name
    return
  end
  for env_file in ~/.codex/.env ~/.env
    if not test -r $env_file
      continue
    end
    set -l env_line (string match -r "^$env_name=.*" < $env_file)
    if test -z "$env_line"
      continue
    end
    set -l env_value (string replace -r "^$env_name=" '' -- $env_line)
    set env_value (string trim -c "'" -- (string trim -c '"' -- $env_value))
    if test -n "$env_value"
      set -gx $env_name $env_value
      return
    end
  end
end
__claudex_export_home_env SAKANA_AI_PRO_API_KEY
__claudex_export_home_env EXA_API_KEY
