import Foundation
import Testing

@testable import PlamoCore

/// The timing marks are diagnostic output, so the test pins the format and both branches rather
/// than the numbers.
@Test func marksOneLinePerPhaseOnlyWhenEnabled() {
    var written = [String]()

    Timing.mark("phase", enabled: false, sink: .capture { written.append($0) })
    #expect(written.isEmpty)

    Timing.mark("phase", enabled: true, sink: .capture { written.append($0) })
    #expect(written.count == 1)
    #expect(written.first?.hasPrefix("plamo-swift timing: t+") == true)
    #expect(written.first?.hasSuffix(" phase\n") == true)

    // The default sink is stderr: one line proves it accepts output without throwing.
    Timing.mark("phase", enabled: true)
    Timing.mark("noop")
    #expect(Timing.enabled == (ProcessInfo.processInfo.environment["PLAMO_TIMING"] != nil))
}
