private let sleepCommand: StaticString = "/usr/bin/pmset"
private let sleepArgument: StaticString = "displaysleepnow"
private let sleepArgumentCount = 3

private let wakeCommand: StaticString = "/usr/bin/caffeinate"
private let wakeUserActiveArgument: StaticString = "-u"
private let wakeTimeoutArgument: StaticString = "-t"
private let wakeDurationArgument: StaticString = "2"
private let wakeArgumentCount = 5

@inline(__always)
private func spawnProcess(
  path: UnsafePointer<CChar>,
  arguments: UnsafePointer<UnsafeMutablePointer<CChar>?>
) {
  // posix_spawn writes the child PID; the environment itself is immutable and empty.
  var childProcessID: Int32 = 0
  let emptyEnvironment: UnsafeMutablePointer<CChar>? = nil
  withUnsafePointer(to: emptyEnvironment) { environment in
    // This is intentionally best-effort. Keeping the watcher alive lets a later
    // lid transition retry if a transient process launch fails.
    _ = spawn(&childProcessID, path, nil, nil, arguments, environment)
  }
}

@inline(__always)
private func spawnCommand<Arguments>(
  path: UnsafePointer<CChar>,
  arguments: borrowing Arguments,
  count: Int
) {
  // Keep the immutable stack tuple alive for the synchronous spawn call.
  withUnsafePointer(to: arguments) { argumentStorage in
    argumentStorage.withMemoryRebound(
      to: Optional<UnsafeMutablePointer<CChar>>.self,
      capacity: count
    ) { argumentVector in
      spawnProcess(path: path, arguments: argumentVector)
    }
  }
}

private func sleepDisplays() {
  let path = cString(sleepCommand)
  let arguments = (
    UnsafeMutablePointer(mutating: path),
    UnsafeMutablePointer(mutating: cString(sleepArgument)),
    Optional<UnsafeMutablePointer<CChar>>.none
  )
  spawnCommand(path: path, arguments: arguments, count: sleepArgumentCount)
}

private func wakeDisplays() {
  let path = cString(wakeCommand)
  let arguments = (
    UnsafeMutablePointer(mutating: path),
    UnsafeMutablePointer(mutating: cString(wakeUserActiveArgument)),
    UnsafeMutablePointer(mutating: cString(wakeTimeoutArgument)),
    UnsafeMutablePointer(mutating: cString(wakeDurationArgument)),
    Optional<UnsafeMutablePointer<CChar>>.none
  )
  spawnCommand(path: path, arguments: arguments, count: wakeArgumentCount)
}

internal func setDisplaySleeping(_ sleeping: Bool) {
  guard sleeping else {
    wakeDisplays()
    return
  }
  sleepDisplays()
}
