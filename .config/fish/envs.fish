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
