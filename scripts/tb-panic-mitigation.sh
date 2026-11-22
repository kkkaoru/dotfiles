#!/bin/bash
#
# Apply or undo the mitigations tried for the recurring ACIO0 Thunderbolt firmware panic
# (assert_id=2548) on the Mac with the OWC Thunderbolt 5 Hub attached:
#   - High Power Mode off  (pmset -a powermode 0)
#   - IP over Thunderbolt off  ("Thunderbolt Bridge")
#   - power-state churn off  (standby, autopoweroff, hibernation, wake-on-LAN, Power Nap)
# None is a proven fix: the assert is inside Apple's Thunderbolt/USB4 firmware, so this only
# removes power margin, an idle network service, and the deep-sleep transitions that drive the
# ACIO power states. Thunderbolt hubs, docks and their USB, DisplayPort and PCIe tunnelling are
# unaffected by the bridge change.
#
# The power tier exists because 5 of 9 panics fell between 23:00 and 03:20, i.e. in the idle
# window where the Mac enters standby; the hub's JHL9580 controller is documented churning
# power states while idle. Cost: no hibernate, no wake for network access, no Power Nap.
#
#   ./scripts/tb-panic-mitigation.sh                # print the current state
#   ./scripts/tb-panic-mitigation.sh apply          # asks for an admin password
#   ./scripts/tb-panic-mitigation.sh revert         # back to High Power Mode and Thunderbolt Bridge
#   ./scripts/tb-panic-mitigation.sh power          # remove deep-sleep and wake-on-LAN transitions
#   ./scripts/tb-panic-mitigation.sh power-revert   # restore the macOS defaults

set -euo pipefail

usage="usage: $0 [status|apply|revert|power|power-revert]"
action=${1:-status}
if [ $# -gt 1 ]; then
  echo "$usage" >&2
  exit 2
fi

case "$action" in
  status) ;;
  apply | revert | power | power-revert)
    # pmset and networksetup need root; re-run this script once as root instead of sudo-ing each call.
    if [ "$(id -u)" -ne 0 ]; then
      exec sudo -- "$0" "$action"
    fi
    ;;
  *)
    echo "$usage" >&2
    exit 2
    ;;
  esac

tb_bridge_state() {
  local line
  line=$(networksetup -listallnetworkservices | grep -x -e 'Thunderbolt Bridge' -e '\*Thunderbolt Bridge' || true)
  case "$line" in
    '') echo absent ;;
    \**) echo off ;;
    *) echo on ;;
  esac
}

set_powermode() {
  pmset -a powermode "$1" || echo "warning: this Mac rejected powermode $1" >&2
}

set_tb_bridge() {
  if [ "$(tb_bridge_state)" = absent ]; then
    echo "no Thunderbolt Bridge service on this Mac; skipped"
    return 0
  fi
  networksetup -setnetworkserviceenabled "Thunderbolt Bridge" "$1"
}

power_state() {
  pmset -g | awk '$1 == "standby" || $1 == "autopoweroff" || $1 == "hibernatemode" ||
    $1 == "womp" || $1 == "powernap" || $1 == "tcpkeepalive" { printf "%s=%s ", $1, $2 }'
  echo
}

set_power() {
  pmset -a standby "$1" autopoweroff "$2" hibernatemode "$3" womp "$4" powernap "$5" tcpkeepalive "$6" ||
    echo "warning: this Mac rejected one of the power settings" >&2
}

case "$action" in
  status)
    echo "powermode: $(pmset -g | awk '$1 == "powermode" { print $2 }')"
    echo "Thunderbolt Bridge: $(tb_bridge_state)"
    echo "power: $(power_state)"
    ;;
  apply)
    set_powermode 0
    set_tb_bridge off
    ;;
  revert)
    set_powermode 2
    set_tb_bridge on
    ;;
  power)
    set_power 0 0 0 0 0 0
    ;;
  power-revert)
    set_power 1 1 3 1 1 1
    ;;
esac

if [ "$action" != status ]; then
  "$0" status
fi
