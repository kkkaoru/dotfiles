import Foundation

internal struct MonitorStore: Sendable {
  private static let directoryPermissions = 0o700
  private static let filePermissions = 0o600
  private static let lightActionCooldown: TimeInterval = 60

  internal let stateDirectory: URL
  internal let signatureURL: URL
  internal let lastLightActionURL: URL
  internal let lastHealthDecisionURL: URL
  internal let transactionLockURL: URL

  internal init(home: URL) {
    stateDirectory = home.appending(
      path: "Library/Application Support/com.kkkaoru.mac-wifi-stability",
      directoryHint: .isDirectory
    )
    signatureURL = stateDirectory.appending(path: "network.signature")
    lastLightActionURL = stateDirectory.appending(path: "last-light-action.epoch")
    lastHealthDecisionURL = stateDirectory.appending(path: "last-health-decision.signature")
    transactionLockURL = stateDirectory.appending(path: "connection.transaction.lock")
  }

  internal func prepare() throws {
    try FileManager.default.createDirectory(
      at: stateDirectory,
      withIntermediateDirectories: true,
      attributes: [.posixPermissions: Self.directoryPermissions]
    )
    try FileManager.default.setAttributes(
      [.posixPermissions: Self.directoryPermissions],
      ofItemAtPath: stateDirectory.path(percentEncoded: false)
    )
  }

  internal func readSignature() -> String? {
    guard let data = try? Data(contentsOf: signatureURL) else {
      return nil
    }
    let signature = (String(bytes: data, encoding: .utf8) ?? "")
      .trimmingCharacters(in: .whitespacesAndNewlines)
    return signature.isEmpty ? nil : signature
  }

  internal func saveSignature(_ signature: String) throws {
    try Data("\(signature)\n".utf8).write(to: signatureURL, options: .atomic)
    try FileManager.default.setAttributes(
      [.posixPermissions: Self.filePermissions],
      ofItemAtPath: signatureURL.path(percentEncoded: false)
    )
  }

  internal func lightActionIsAllowed() -> Bool {
    guard let last = readEpoch(from: lastLightActionURL) else {
      return true
    }
    return Date().timeIntervalSince1970 - last >= Self.lightActionCooldown
  }

  internal func recordLightAction() throws {
    try saveEpoch(Date().timeIntervalSince1970, to: lastLightActionURL)
  }

  internal func healthDecisionIsAllowed(for signature: String) -> Bool {
    readText(from: lastHealthDecisionURL) != signature
  }

  internal func recordHealthDecision(for signature: String) throws {
    try saveText(signature, to: lastHealthDecisionURL)
  }

  internal func acquireTransactionLock() -> ProcessLock? {
    let lock = ProcessLock(url: transactionLockURL)
    if lock != nil {
      try? FileManager.default.setAttributes(
        [.posixPermissions: Self.filePermissions],
        ofItemAtPath: transactionLockURL.path(percentEncoded: false)
      )
    }
    return lock
  }

  private func saveEpoch(_ epoch: TimeInterval, to url: URL) throws {
    try Data("\(Int(epoch))\n".utf8).write(to: url, options: .atomic)
    try FileManager.default.setAttributes(
      [.posixPermissions: Self.filePermissions],
      ofItemAtPath: url.path(percentEncoded: false)
    )
  }

  private func readEpoch(from url: URL) -> TimeInterval? {
    guard let data = try? Data(contentsOf: url),
      let string = String(bytes: data, encoding: .utf8),
      let value = Double(string.trimmingCharacters(in: .whitespacesAndNewlines))
    else {
      return nil
    }

    return value
  }

  private func saveText(_ value: String, to url: URL) throws {
    try Data("\(value)\n".utf8).write(to: url, options: .atomic)
    try FileManager.default.setAttributes(
      [.posixPermissions: Self.filePermissions],
      ofItemAtPath: url.path(percentEncoded: false)
    )
  }

  private func readText(from url: URL) -> String? {
    guard let data = try? Data(contentsOf: url) else {
      return nil
    }
    let value = (String(bytes: data, encoding: .utf8) ?? "")
      .trimmingCharacters(in: .whitespacesAndNewlines)
    return value.isEmpty ? nil : value
  }
}
