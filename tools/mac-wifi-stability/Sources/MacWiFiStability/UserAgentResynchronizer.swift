import Foundation

internal struct UserAgentResynchronizer: Sendable {
  internal let runner: CommandRunner
  internal let uid: String

  internal func light() -> [String] {
    [
      restart(label: "com.apple.wifi.WiFiAgent", processToken: "WiFiAgent"),
      restart(label: "com.apple.networkserviceproxy", processToken: "networkserviceproxy"),
    ]
  }

  internal func full() -> [String] {
    // sharingd owns Bonjour sharing-name registration. Restarting it during a
    // network transition can make macOS treat its previous registration as a
    // name collision and persist a suffixed ComputerName (for example, "Mac (2)").
    // It is unrelated to IP path recovery, so deliberately leave it running.
    light() + [flushDNSCache()]
  }

  private func restart(label: String, processToken: String) -> String {
    let service = "gui/\(uid)/\(label)"
    let printed = runner.run("/bin/launchctl", arguments: ["print", service])
    guard printed.succeeded,
      let pid = pid(from: printed.stdout)
    else {
      return "action=user-agent-restart label=\(label) result=skip reason=no-pid"
    }

    let owner = runner.run("/bin/ps", arguments: ["-p", pid, "-o", "uid="]).stdout
      .trimmingCharacters(in: .whitespacesAndNewlines)
    guard owner == uid else {
      return "action=user-agent-restart label=\(label) result=skip reason=owner-mismatch"
    }

    let command = runner.run("/bin/ps", arguments: ["-p", pid, "-o", "command="]).stdout
    guard command.contains(processToken) else {
      return "action=user-agent-restart label=\(label) result=skip reason=process-mismatch"
    }

    let killed = runner.run("/bin/kill", arguments: ["-TERM", pid]).succeeded
    return "action=user-agent-restart label=\(label) result=\(killed ? "ok" : "failed") pid=\(pid)"
  }

  private func flushDNSCache() -> String {
    let result = runner.run("/usr/bin/dscacheutil", arguments: ["-flushcache"])
    return "action=dns-cache-flush result=\(result.succeeded ? "ok" : "failed")"
  }

  private func pid(from output: String) -> String? {
    for line in output.split(whereSeparator: \.isNewline) {
      let trimmed = line.trimmingCharacters(in: .whitespaces)
      guard trimmed.hasPrefix("pid =") else { continue }
      let value = trimmed.dropFirst("pid =".count).trimmingCharacters(in: .whitespaces)
      guard value.allSatisfy(\.isNumber), !value.isEmpty else {
        return nil
      }
      return String(value)
    }

    return nil
  }
}
