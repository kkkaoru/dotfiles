# ACIO0 Thunderbolt firmware panic (assert_id=2548)

Recurring kernel panic on this Mac with the OWC Thunderbolt 5 Hub attached. Cause is **not**
established; the strongest reading of the evidence is a Thunderbolt/USB4 firmware handshake or
power-state fault on the link between Apple's ACIO firmware and the hub.

| | |
|---|---|
| Status | open — mitigated, not fixed |
| First seen | 2026-11-17 11:35 JST |
| Last seen | 2026-11-22 01:57 JST |
| Panics recorded | 9, all `assert_id=2548` |
| Pi session | `01a1fcbf-4047-7717-a54d-ff135dcda229` |

Resume the investigation session with:

```
pi --session 01a1fcbf-4047-7717-a54d-ff135dcda229
```

Transcript:
`~/.pi/agent/sessions/--Users-kkk4oru-ghq-github.com-kkkaoru-dotfiles--/2026-11-21T06-15-54-696Z_01a1fcbf-4047-7717-a54d-ff135dcda229.jsonl`

## Panic

```
panic(cpu 12 caller 0xfffffe0039d411d4): ACIO0 Recoverable Panic -  assert_id=2548 - ACIO main workloop(2)
 assert_id=2548
RTKit: RTKit_armv7m_Oz_floatabisoft-3514.41.1.release - Client: AppleCIOFirmwareV2-761.40.2~1487__2026-10-07-21:52:29-PDT__t6050
```

- `ACIO` is the Apple Converged IO block that owns Thunderbolt/USB4; the faulting task is inside
  Apple's own RTKit firmware image, not in macOS or in a kext.
- `assert_id=2548` in `ACIO main workloop(2)` recurs with the same id and the same firmware build
  every time, which is why this is treated as one bug rather than several.
- The panic is reported as *Recoverable* yet still takes the machine down.

## Machine and hub

| | |
|---|---|
| Host | `Mac17,9`, Apple M5 Pro, macOS 27.2.1 (26B131), xnu-13432.41.4~25 |
| Host TB firmware | `AppleCIOFirmwareV2-761.40.2~1487` |
| Hub | OWC Thunderbolt 5 Hub, model id 56961, revision 1 |
| Hub firmware | `ROM Version 98`, `EEPROM Revision 2` |
| Hub controller | Intel `JHL9580` |

Apple's driver carries a hub-specific workaround, which shows the combination is known to be
fiddly: `IOThunderboltSwitchIntelJHL9580::overrideSupportedCLxStates - Gen4 disabling clx`.

## Panic inventory

`/Library/Logs/DiagnosticReports/panic-full-*.panic`:

| time (JST) | assert |
|---|---|
| 2026-11-17 11:35 | 2548 |
| 2026-11-18 23:05 | 2548 |
| 2026-11-19 01:20 | 2548 |
| 2026-11-21 00:16 | 2548 |
| 2026-11-21 03:10 | 2548 |
| 2026-11-21 14:07 | 2548 |
| 2026-11-21 15:13 | 2548 |
| 2026-11-21 23:20 | 2548 |
| 2026-11-22 01:57 | 2548 |

## Considered and not supported

- **Thermals** — `pmset -g therm` recorded no warnings during the incident window.
- **Load** — load average ~9, nothing that should reach firmware.
- **Software churn** — Parallels (`prl_disp_service`, `prl_naptd`) and `KensingtonWorksHelper` are
  resident; nothing links them to the ACIO workloop.
- **Memory / storage** — the panic string carries no memory or I/O error.

Correlation, not proof: every panic happened with the hub in the chain. The hub has not yet been
taken out of the chain for a controlled period, and has not been tried on another Mac.

## Mitigations applied

`./scripts/tb-panic-mitigation.sh [status|apply|revert|power|power-revert]`:

- `pmset -a powermode 0` — High Power Mode off, removing power margin from the link.
- Thunderbolt Bridge network service disabled — removes an idle IP-over-Thunderbolt service.
  Thunderbolt tunnelling (USB, DisplayPort, PCIe) is unaffected.
- `power` — `standby`, `autopoweroff`, `hibernatemode`, `womp`, `powernap` and `tcpkeepalive`
  all 0, so the Mac neither enters deep standby/hibernation nor wakes for network access.
  `pmset` warns that disabling TCP keep-alive while asleep stops "Find My Mac" from working.

Neither of the first two is a proven fix. **The 2026-11-22 01:57 panic happened with both of
those active** (`powermode 0` set at 23:43, bridge disabled), so they do not prevent it — the
firmware assert survives the power-margin and idle-network changes.

The power tier targets the one dimension still under control: the deep-sleep transitions that
drive ACIO power states. Five of the nine panics fell between 23:00 and 03:20, the idle window
where the Mac enters standby, and the hub's `JHL9580` controller is documented churning power
states while idle with no downstream devices. Cost: no hibernate, no wake for network access and
no Power Nap; normal display sleep and Thunderbolt tunnelling are unchanged.

Current state (applied 2026-11-22 15:4x JST): `powermode 0`, `Thunderbolt Bridge` disabled,
`standby=0 powernap=0 hibernatemode=0 tcpkeepalive=0 womp=0`. `Find My Mac` is degraded while the
Mac sleeps, and there is no hibernate image if the battery runs flat in sleep.

After the 01:57 reboot `tb-watch.log` recorded `hub=no` for the first hour, then `hub=yes` again
at `ROM 98`, `speed=2,8`, `width=1,2`: the hub was out of the chain for that hour and the machine
stayed up, which is consistent with the correlation but is not a controlled test.

## Evidence collection

`./scripts/tb-watch.sh [log|show|tail|install|uninstall]` appends one line every 5 minutes to
`~/Library/Logs/tb-watch.log` (launchd agent `com.kkkaoru.tb-watch`, background priority):

```
2026-11-21T16:47:02Z boot=ED94DD88 up=146m hub=yes rom=98 speed=2,8 width=1,2 panics=8 last=2026-11-21-232029
```

What to look for:

- `boot=` changes plus `panics=` +1 → a panic happened; the same line says whether the hub was
  attached and at what link speed.
- `hub=` flipping to `no` and back while nobody touches the cable → the hub is dropping off the
  bus on its own. That is the one finding that would justify an RMA and that the panic reports
  cannot show. A 1-second sampler already caught `0 0 0 1 1 0 0 0` once, unattributed.
- `speed=` / `width=` dropping to a lower set → cable or port rather than firmware.

## Hub firmware

No public firmware update exists for this hub: OWC publishes no updater for it, `software.owc.com`
lists only Dock Ejector and Innergize, and the support manual only notes that behaviour may depend
on firmware version. `ROM 98` / `EEPROM 2` is what ships.

The only route is an OWC support case asking whether a newer revision exists, whether JHL9580
firmware is field-updatable at all, and — if not — for advance replacement. Supporting datapoint
for the ticket: an Intel community report of the same hub family churning power states every ~5 s
while idle with no downstream devices (Windows host, `JHL9480`).

Even a hub update would only address half of it: the assert is in Apple's ACIO firmware, so a
macOS update is the other half.

## Next steps

1. Watch `tb-watch.log` for a week with the power tier applied: does the panic interval lengthen?
   Revert with `power-revert` if wake-on-LAN, Find My Mac or hibernate behaviour is missed.
2. Read `tb-watch.log` after two or three days: is `hub=` stable, and does it flap on its own?
3. Decide the controlled test: hub out of the chain for a day, or the hub on another Mac.
4. Open the OWC case with the panic files, `ROM 98` / `EEPROM 2`, and the idle-power-state report.
5. Retest after the next macOS update and check whether `AppleCIOFirmwareV2` moved past
   `761.40.2~1487`.

## Artifacts

- Panic files: `/Library/Logs/DiagnosticReports/panic-full-*.panic` (not in Git)
- Raw dumps from the first investigation: `tmp/panic-report.txt`, `tmp/panic-report2.txt`,
  `tmp/panic-report3.txt` (`tmp/` is gitignored)
- Scripts: `scripts/tb-panic-mitigation.sh`, `scripts/tb-watch.sh`
- Log: `~/Library/Logs/tb-watch.log`
