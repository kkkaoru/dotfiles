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

/// Counts the lukewarm entries without stopping the server: the weights go, the process stays.
actor LukewarmHits {
    private(set) var count = 0

    var isEmpty: Bool { count < 1 }

    func record() {
        count += 1
    }
}

@Test func releasesTheWeightsOnceAfterAShorterIdle() async throws {
    let directory = try Fixtures.temporaryDirectory("lukewarm")
    let hits = LukewarmHits()
    let box = ServerBox()
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(
            port: nil,
            socketPath: directory.appendingPathComponent("plamo.sock").path,
            idleSeconds: 3,
            lukewarmSeconds: 0.1,
            idleCheckInterval: 0.05),
        onLukewarm: { await hits.record() },
        onIdle: { box.stop() })
    box.server = server
    _ = try await server.start()
    // Wait for the release instead of racing the idle exit: the idle exit is 3 s and the release
    // 0.1 s, so a loaded machine can starve the watcher for a while without skipping the release.
    let deadline = Date().addingTimeInterval(5)
    while await hits.isEmpty && Date() < deadline {
        try await Task.sleep(for: .milliseconds(20))
    }
    await server.waitUntilStopped()
    // Once per idle stretch: a second release would find nothing and only wake the runtime. The
    // 3 s idle window keeps ticking after the release, so a repeat would show up here.
    let releases = await hits.count
    #expect(releases == 1)
}
