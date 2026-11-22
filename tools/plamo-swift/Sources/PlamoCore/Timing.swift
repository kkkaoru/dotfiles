import Foundation

/// One line per phase on stderr when `PLAMO_TIMING` is set, off otherwise for the price of one
/// branch. Start-up and first-request numbers move by tens of milliseconds, so a change is worth
/// keeping only when the phase it shortens is visible here.
///
/// `Date` rather than `DispatchTime`: a `DispatchTime.now()` in a static initializer trapped the
/// process inside libdispatch's one-time `timebaseInfo` setup, before any line was written.
public enum Timing {
    /// Where a mark goes: stderr in production, a test's own sink otherwise.
    public enum Sink {
        /// One line on stderr, the production sink.
        case standardError
        /// A test's own sink, so the format is checked without touching stderr.
        case capture((String) -> Void)
    }

    private static let state = (
        enabled: ProcessInfo.processInfo.environment["PLAMO_TIMING"] != nil,
        start: Date()
    )

    /// Whether `PLAMO_TIMING` was set when the process started.
    public static var enabled: Bool { state.enabled }

    /// Milliseconds since the process started, printed as `t+  12.34 name`. `enabled` and `sink`
    /// default to the environment and stderr, and a test passes its own to drive both branches.
    public static func mark(
        _ name: String, enabled: Bool? = nil, sink: Sink = .standardError
    ) {
        guard enabled ?? state.enabled else { return }
        let line = line(for: name, since: state.start)
        switch sink {
        case .standardError: write(line)
        case .capture(let record): record(line)
        }
    }

    internal static func line(for name: String, since start: Date) -> String {
        let elapsed = Date().timeIntervalSince(start) * 1_000
        return "plamo-swift timing: t+" + String(format: "%8.2f", elapsed) + " " + name + "\n"
    }

    /// The default sink: one line on stderr, unbuffered so it interleaves with the server's log.
    private static func write(_ line: String) {
        FileHandle.standardError.write(Data(line.utf8))
    }
}
