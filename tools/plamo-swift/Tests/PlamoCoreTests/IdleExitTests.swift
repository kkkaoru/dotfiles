import Foundation
import Testing

@testable import PlamoCore

/// Holds the server so the idle callback can stop it: `onIdle` cannot capture the server it belongs
/// to at construction time.
final class ServerBox: @unchecked Sendable {
    var server: PlamoServer?

    func stop() {
        guard let server else { return }
        Task { await server.stop() }
    }
}

@Test func stopsAfterAnIdleInterval() async throws {
    let directory = try Fixtures.temporaryDirectory("idle")
    let box = ServerBox()
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(
            port: nil,
            socketPath: directory.appendingPathComponent("plamo.sock").path,
            idleSeconds: 0.2,
            idleCheckInterval: 0.1),
        onIdle: { box.stop() })
    box.server = server
    _ = try await server.start()
    // Before the fix the watcher gave up on its first non-expired check and the continuation behind
    // waitUntilStopped was never resumed, so the process outlived its idle timeout.
    let started = Date()
    await server.waitUntilStopped()
    #expect(Date().timeIntervalSince(started) < 5)
}
