# ACIO0 Thunderbolt firmware panic (assert_id=2548)

Recurring kernel panic on this Mac with the OWC Thunderbolt 5 Hub attached. Cause is **not**
established; the strongest reading of the evidence is a Thunderbolt/USB4 firmware handshake or
power-state fault on the link between Apple's ACIO firmware and the hub.

| | |
|---|---|
| Status | open — mitigated, not fixed |
| First seen | 2026-11-17 11:35 JST |
| Last seen | 2026-11-21 23:20 JST |
| Panics recorded | 8, all `assert_id=2548` |
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

## Considered and not supported

- **Thermals** — `pmset -g therm` recorded no warnings during the incident window.
- **Load** — load average ~9, nothing that should reach firmware.
- **Software churn** — Parallels (`prl_disp_service`, `prl_naptd`) and `KensingtonWorksHelper` are
  resident; nothing links them to the ACIO workloop.
- **Memory / storage** — the panic string carries no memory or I/O error.

Correlation, not proof: every panic happened with the hub in the chain. The hub has not yet been
taken out of the chain for a controlled period, and has not been tried on another Mac.

## Mitigations applied

`./scripts/tb-panic-mitigation.sh [status|apply|revert]`:

- `pmset -a powermode 0` — High Power Mode off, removing power margin from the link.
- Thunderbolt Bridge network service disabled — removes an idle IP-over-Thunderbolt service.
  Thunderbolt tunnelling (USB, DisplayPort, PCIe) is unaffected.

Neither is a proven fix. Current state: `powermode 0`, `Thunderbolt Bridge` disabled.

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

1. Read `tb-watch.log` after two or three days: is `hub=` stable, and does it flap on its own?
2. Decide the controlled test: hub out of the chain for a day, or the hub on another Mac.
3. Open the OWC case with the panic files, `ROM 98` / `EEPROM 2`, and the idle-power-state report.
4. Retest after the next macOS update and check whether `AppleCIOFirmwareV2` moved past
   `761.40.2~1487`.

## Artifacts

- Panic files: `/Library/Logs/DiagnosticReports/panic-full-*.panic` (not in Git)
- Raw dumps from the first investigation: `tmp/panic-report.txt`, `tmp/panic-report2.txt`
  (`tmp/` is gitignored)
- Scripts: `scripts/tb-panic-mitigation.sh`, `scripts/tb-watch.sh`
- Log: `~/Library/Logs/tb-watch.log`
