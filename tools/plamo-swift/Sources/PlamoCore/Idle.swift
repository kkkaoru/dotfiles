import Foundation

/// When the server has been idle long enough to exit. Kept separate from the timer so the rule can
/// be tested without waiting ten minutes.
public struct IdleTracker: Sendable, Equatable {
    public private(set) var lastActivity: Date
    public private(set) var busy: Bool

    public init(now: Date = Date()) {
        lastActivity = now
        busy = false
    }

    public mutating func touch(now: Date = Date()) {
        lastActivity = now
    }

    public mutating func setBusy(_ busy: Bool) {
        self.busy = busy
    }

    /// Never exits mid-request: the mods hold a request open for the whole decode.
    public func shouldExit(now: Date, idleSeconds: Double) -> Bool {
        !busy && now.timeIntervalSince(lastActivity) > idleSeconds
    }
}
