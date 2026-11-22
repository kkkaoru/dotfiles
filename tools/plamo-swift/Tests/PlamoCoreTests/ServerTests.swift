import Foundation
import Testing

@testable import PlamoCore

/// A tiny blocking HTTP client: the server tests need to see what actually crosses the socket,
/// including a second request on the same connection.
private func connectTCP(port: UInt16) throws -> Int32 {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    guard fd >= 0 else { throw PlamoError.connectionClosed }
    var address = sockaddr_in()
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = port.bigEndian
    address.sin_addr.s_addr = inet_addr("127.0.0.1")
    let size = socklen_t(MemoryLayout<sockaddr_in>.size)
    let result = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Foundation.connect(fd, $0, size)
        }
    }
    guard result == 0 else {
        close(fd)
        throw PlamoError.connectionClosed
    }
    return fd
}

private func connectUnix(path: String) throws -> Int32 {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw PlamoError.connectionClosed }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    let pathBytes = Array(path.utf8)
    withUnsafeMutableBytes(of: &address.sun_path) { buffer in
        buffer.copyBytes(from: pathBytes.prefix(buffer.count - 1))
    }
    let size = socklen_t(MemoryLayout<sockaddr_un>.size)
    let result = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Foundation.connect(fd, $0, size)
        }
    }
    guard result == 0 else {
        close(fd)
        throw PlamoError.connectionClosed
    }
    return fd
}

private func send(_ fd: Int32, _ text: String) throws {
    let bytes = Array(text.utf8)
    var written = 0
    while written < bytes.count {
        let n = bytes.withUnsafeBytes { buffer in
            write(fd, buffer.baseAddress!.advanced(by: written), bytes.count - written)
        }
        guard n > 0 else { throw PlamoError.connectionClosed }
        written += n
    }
}

/// Reads one HTTP response: headers up to the blank line, then `Content-Length` bytes.
private func readResponse(_ fd: Int32) throws -> String {
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 4_096)
    func headerEnd() -> Int? {
        data.range(of: Data("\r\n\r\n".utf8)).map { $0.lowerBound + 4 }
    }
    while headerEnd() == nil {
        let n = read(fd, &buffer, buffer.count)
        guard n > 0 else { throw PlamoError.connectionClosed }
        data.append(contentsOf: buffer[0..<n])
    }
    let end = headerEnd()!
    let head = String(decoding: data.prefix(end), as: UTF8.self)
    var length = 0
    for line in head.split(separator: "\r\n") where line.lowercased().hasPrefix("content-length:") {
        length = Int(line.split(separator: ":")[1].trimmingCharacters(in: .whitespaces)) ?? 0
    }
    while data.count - end < length {
        let n = read(fd, &buffer, buffer.count)
        guard n > 0 else { throw PlamoError.connectionClosed }
        data.append(contentsOf: buffer[0..<n])
    }
    return String(decoding: data, as: UTF8.self)
}

/// Port 0 lets the kernel pick one; `start()` reports the port it bound.
private let anyPort: UInt16 = 0

/// Holds a port so the server's own bind has to fail, and reports the port it took.
private func occupyPort() throws -> (fd: Int32, port: UInt16) {
    let fd = socket(AF_INET, SOCK_STREAM, 0)
    guard fd >= 0 else { throw PlamoError.connectionClosed }
    var address = sockaddr_in()
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = 0
    address.sin_addr.s_addr = inet_addr("127.0.0.1")
    let size = socklen_t(MemoryLayout<sockaddr_in>.size)
    let bound = withUnsafeMutablePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Foundation.bind(fd, $0, size) }
    }
    guard bound == 0, listen(fd, 1) == 0 else {
        close(fd)
        throw PlamoError.connectionClosed
    }
    var actual = sockaddr_in()
    var actualSize = size
    _ = withUnsafeMutablePointer(to: &actual) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            getsockname(fd, $0, &actualSize)
        }
    }
    return (fd, UInt16(bigEndian: actual.sin_port))
}

/// The TCP port out of "127.0.0.1:1234 and /tmp/plamo.sock": a second endpoint must not hide it.
private func boundPort(_ endpoints: String) -> UInt16? {
    let prefix = "127.0.0.1:"
    for part in endpoints.split(separator: " and ") where part.hasPrefix(prefix) {
        return UInt16(part.dropFirst(prefix.count))
    }
    return nil
}

@Test func servesTwoRequestsOnOneTCPConnection() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let fd = try connectTCP(port: try #require(boundPort(endpoints)))
    defer { close(fd) }
    try send(fd, "GET /v1/models HTTP/1.1\r\nHost: localhost\r\n\r\n")
    let first = try readResponse(fd)
    #expect(first.contains("200 OK"))
    #expect(first.contains("plamo-3-translate"))
    let body = #"{"prompt":"hi"}"#
    try send(
        fd,
        "POST /v1/completions HTTP/1.1\r\nContent-Length: \(body.utf8.count)\r\n\r\n\(body)")
    let second = try readResponse(fd)
    #expect(second.contains("200 OK"))
    #expect(second.contains("text_completion"))
    await server.stop()
}

@Test func servesAUnixSocketRequestAndRefusesNobody() async throws {
    // sun_path is 104 bytes on macOS, and NWListener reports ready even when bind failed, so a long
    // temporary path produces a listener with no socket file at all.
    let path = "/tmp/plamo-swift-test-\(UUID().uuidString).sock"
    defer { try? FileManager.default.removeItem(atPath: path) }
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: nil, socketPath: path, idleSeconds: 0))
    let endpoints = try await server.start()
    #expect(endpoints.contains(path))
    let attributes = try FileManager.default.attributesOfItem(atPath: path)
    #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
    let fd = try connectUnix(path: path)
    defer { close(fd) }
    try send(fd, "GET /v1/models HTTP/1.1\r\nHost: local\r\n\r\n")
    #expect(try readResponse(fd).contains("plamo-3-translate"))
    await server.stop()
}

@Test func answersAGarbageRequestWith400() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let fd = try connectTCP(port: try #require(boundPort(endpoints)))
    defer { close(fd) }
    try send(fd, "nonsense\r\n\r\n")
    let response = try readResponse(fd)
    #expect(response.contains("400 Bad Request"))
    await server.stop()
}

@Test func refusesASecondStartAndAnEmptyConfiguration() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    _ = try await server.start()
    await #expect(throws: PlamoError.self) { _ = try await server.start() }
    await server.stop()

    let empty = PlamoServer(
        router: Router(provider: StubProvider()), configuration: .init(port: nil))
    await #expect(throws: PlamoError.self) { _ = try await empty.start() }
}

@Test func refusesAPortSomebodyElseHolds() async throws {
    let (fd, port) = try occupyPort()
    defer { close(fd) }
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: port, idleSeconds: 0))
    await #expect(throws: PlamoError.self) { _ = try await server.start() }
    // A failed start leaves nothing behind: a second attempt fails the same way.
    await #expect(throws: PlamoError.self) { _ = try await server.start() }
}

@Test func reportsAnUnusableSocketPath() async throws {
    // A directory that does not exist cannot hold the socket file, so the start must fail loudly
    // instead of reporting an endpoint nothing is listening on.
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(
            port: nil, socketPath: "/nonexistent-plamo-\(UUID().uuidString)/plamo.sock",
            idleSeconds: 0))
    await #expect(throws: PlamoError.self) { _ = try await server.start() }
}

@Test func refusesAnOversizedRequestHead() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let fd = try connectTCP(port: try #require(boundPort(endpoints)))
    defer { close(fd) }
    // One header line past the 4 MB cap: the server must answer 400 instead of buffering it.
    try send(fd, "GET /v1/models HTTP/1.1\r\nX-Pad: ")
    try send(fd, String(repeating: "a", count: HTTPRequestParser.maximumBodyBytes))
    let response = try readResponse(fd)
    #expect(response.contains("400 Bad Request"))
    await server.stop()
}

@Test func aShortBodyEndsTheConnection() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let fd = try connectTCP(port: try #require(boundPort(endpoints)))
    defer { close(fd) }
    try send(fd, "POST /v1/completions HTTP/1.1\r\nContent-Length: 50\r\n\r\n{}")
    shutdown(fd, SHUT_WR)
    var buffer = [UInt8](repeating: 0, count: 64)
    let read0 = read(fd, &buffer, buffer.count)
    #expect(read0 <= 0)
    await server.stop()
}

@Test func readsABodyThatArrivesAfterTheHead() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let fd = try connectTCP(port: try #require(boundPort(endpoints)))
    defer { close(fd) }
    let body = #"{"prompt":"hi"}"#
    try send(
        fd, "POST /v1/completions HTTP/1.1\r\nContent-Length: \(body.utf8.count)\r\n\r\n")
    // The pause keeps the kernel from coalescing the body into the same read as the head.
    try await Task.sleep(for: .milliseconds(100))
    try send(fd, body)
    #expect(try readResponse(fd).contains("text_completion"))
    await server.stop()
}

@Test func survivesAClientThatResetsTheConnection() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let port = try #require(boundPort(endpoints))
    let fd = try connectTCP(port: port)
    var options = linger(l_onoff: 1, l_linger: 0)
    setsockopt(fd, SOL_SOCKET, SO_LINGER, &options, socklen_t(MemoryLayout<linger>.size))
    try send(fd, "GET /v1/models HTTP/1.1\r\nHost: local\r\n\r\n")
    // Linger 0 turns close into a reset, so the server's send or receive fails.
    close(fd)
    let second = try connectTCP(port: port)
    defer { close(second) }
    try send(second, "GET /v1/models HTTP/1.1\r\nHost: local\r\n\r\n")
    #expect(try readResponse(second).contains("200 OK"))
    await server.stop()
}

@Test func survivesAResetWhileWaitingForTheBody() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    let endpoints = try await server.start()
    let port = try #require(boundPort(endpoints))
    let fd = try connectTCP(port: port)
    try send(fd, "POST /v1/completions HTTP/1.1\r\nContent-Length: 50\r\n\r\n{}")
    // The server is blocked in receive for the rest of the body; a reset must not crash it.
    var options = linger(l_onoff: 1, l_linger: 0)
    setsockopt(fd, SOL_SOCKET, SO_LINGER, &options, socklen_t(MemoryLayout<linger>.size))
    close(fd)
    let second = try connectTCP(port: port)
    defer { close(second) }
    try send(second, "GET /v1/models HTTP/1.1\r\nHost: local\r\n\r\n")
    #expect(try readResponse(second).contains("200 OK"))
    await server.stop()
}

@Test func servesBothEndpointsOfOneConfiguration() async throws {
    let path = "/tmp/plamo-swift-test-\(UUID().uuidString).sock"
    defer { try? FileManager.default.removeItem(atPath: path) }
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, socketPath: path, idleSeconds: 0))
    let endpoints = try await server.start()
    #expect(endpoints.contains(path))
    let fd = try connectTCP(port: try #require(boundPort(endpoints)))
    defer { close(fd) }
    try send(fd, "GET /v1/models HTTP/1.1\r\nHost: local\r\n\r\n")
    #expect(try readResponse(fd).contains("200 OK"))
    await server.stop()
}

@Test func stoppingResumesTheWaiter() async throws {
    let server = PlamoServer(
        router: Router(provider: StubProvider()),
        configuration: .init(port: anyPort, idleSeconds: 0))
    _ = try await server.start()
    let waiter = Task { await server.waitUntilStopped() }
    await server.stop()
    await waiter.value
    // A second stop must not trap on an already resumed continuation.
    await server.stop()
}
