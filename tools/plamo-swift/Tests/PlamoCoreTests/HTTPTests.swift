import Foundation
import Testing

@testable import PlamoCore

@Test func readsAContentLengthFromACRLFHead() throws {
    let head = Data(
        "POST /v1/completions HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\n".utf8)
    #expect(try HTTPRequestParser.contentLength(head: head) == 5)
}

@Test func parsesARequestWithABody() throws {
    let head = Data(
        "POST /v1/completions HTTP/1.1\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n".utf8)
    let request = try HTTPRequestParser.parse(head: head, body: Data("{}".utf8))
    #expect(request.method == "POST")
    #expect(request.path == "/v1/completions")
    #expect(request.keepAlive)
    #expect(request.body == Data("{}".utf8))
}

@Test func rejectsAnOversizedContentLength() {
    let head = Data("POST / HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n".utf8)
    #expect(throws: PlamoError.self) { try HTTPRequestParser.contentLength(head: head) }
}

@Test func rejectsAGarbageRequestLine() {
    let head = Data("nonsense\r\n\r\n".utf8)
    #expect(throws: PlamoError.self) { try HTTPRequestParser.parse(head: head, body: Data()) }
}
