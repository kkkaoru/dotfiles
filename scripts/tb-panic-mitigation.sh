#!/bin/bash
#
# Apply or undo the mitigations tried for the recurring ACIO0 Thunderbolt firmware panic
# (assert_id=2548, "ACIO main workloop") on this Mac with the OWC Thunderbolt 5 Hub attached.
# The assert is inside Apple's Thunderbolt/USB4 controller firmware (AppleCIOFirmwareV2), so nothing
# here can fix it; these tiers only remove conditions that were suspected of provoking it.
#
# What 14 panics (2026-11-17..23) and the kernel log actually show: the hub's upstream link tears
# down and re-enumerates in bursts - `IOThunderboltSwitchType7::handlePlugEventPacket - unplug on
# primary lane - requestPowerDown`, then `checkForCableStateChange - cable connected, bur power down
# requested` / `doze` / `setPhyState - PHY is off` / `we're off, but cable is connected, turn on` -
# 324 hub teardowns in 4h13m, a median 7 s apart, and every panic follows a burst 7-23 s later.
# Nothing in software starts a burst: the kernel log before one shows no power-delivery event, no
# device attach and no app action, only the switch's own unplug packet. macOS powers the port down
# because it believes the cable was pulled while the cable is still there, so the fault is in the
# physical link - connector, cable, hub receptacle or Mac receptacle - and the fix is mechanical.
# A sleep-state theory is ruled out: two panics hit 5 and 10 minutes after boot.
#
# Keeping the hub and its cable, the order to try is: reseat both ends, flip the plug 180 degrees,
# support the cable so its weight cannot lever the connector (OWC sells ClingOn mounts for this),
# then move to another Thunderbolt port on the Mac. `./scripts/tb-watch.sh` records the flap rate as
# `flaps=`; a fixed connection holds flaps=0 for hours. About 1 teardown in 100 panics, so cutting
# the teardown rate cuts the panics in proportion.
#
# Tiers, none proven, in the order they were tried:
#   apply   High Power Mode off + IP over Thunderbolt off
#   power   standby, autopoweroff, hibernation, wake-on-LAN and Power Nap off. Applied 2026-11-22
#           15:32 and five panics followed, two within 10 minutes of boot, so it did not work. It
#           costs hibernate, Power Nap and wake-for-network-access, so `power-revert` is the state
#           to leave this Mac in unless a sleep-related symptom returns.
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
