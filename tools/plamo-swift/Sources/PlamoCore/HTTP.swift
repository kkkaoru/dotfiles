import Foundation

/// The parts of an HTTP request the server acts on. Everything else in the head is ignored.
public struct HTTPRequest: Sendable, Equatable {
    public let method: String
    public let path: String
    public let body: Data
    /// HTTP/1.1 keeps the connection open unless the client asks to close it; HTTP/1.0 the reverse.
    public let keepAlive: Bool

    public init(method: String, path: String, body: Data, keepAlive: Bool = false) {
        self.method = method
        self.path = path
        self.body = body
        self.keepAlive = keepAlive
    }
}

public struct HTTPResponse: Sendable, Equatable {
    public let status: String
    public let body: String

    public init(status: String, body: String) {
        self.status = status
        self.body = body
    }

    /// A keep-alive connection is what makes a socket round trip cheap: the mods' agent reuses it,
    /// so no request pays the connect again.
    public func serialized(keepAlive: Bool = false) -> Data {
        let payload = Data(body.utf8)
        let connection = keepAlive ? "keep-alive" : "close"
        let head = """
            HTTP/1.1 \(status)\r
            Content-Type: application/json\r
            Content-Length: \(payload.count)\r
            Connection: \(connection)\r
            \r

            """
        return Data(head.utf8) + payload
    }
}

/// Parses request bytes. Every failure is a named `PlamoError`, so a malformed request becomes a
/// 400 instead of a crash or a hang.
public enum HTTPRequestParser {
    /// The largest body accepted: a longer `content-length` is a 400.
    public static let maximumBodyBytes = 4 << 20
    private static let terminator = Data("\r\n\r\n".utf8)
    private static let lineBreakText = "\r\n"

    /// Offset just past the blank line, or nil while the head is still incomplete.
    public static func headerEnd(in data: Data) -> Int? {
        guard let range = data.range(of: terminator) else { return nil }
        return data.distance(from: data.startIndex, to: range.upperBound)
    }

    /// The `content-length` header, or 0 when the request carries none.
    public static func contentLength(head: Data) throws -> Int {
        guard let value = header("content-length", in: head) else { return 0 }
        guard let length = Int(value), length >= 0, length <= maximumBodyBytes else {
            throw PlamoError.invalidRequest(reason: "content-length \(value)")
        }
        return length
    }

    /// Splits a complete head and its body into a request.
    public static func parse(head: Data, body: Data) throws -> HTTPRequest {
        guard var text = String(bytes: head, encoding: .utf8) else {
            throw PlamoError.invalidRequest(reason: "request head is not UTF-8")
        }
        if text.hasSuffix("\r\n\r\n") { text.removeLast(4) }
        let lines = text.components(separatedBy: lineBreakText)
        guard let requestLine = lines.first else {
            throw PlamoError.invalidRequest(reason: "empty request")
        }
        let parts = requestLine.split(separator: " ")
        guard parts.count >= 2 else { throw PlamoError.invalidRequest(reason: "request line") }
        if parts.count >= 3, !parts[2].hasPrefix("HTTP/") {
            throw PlamoError.invalidRequest(reason: "request line")
        }
        let version = parts.count >= 3 ? String(parts[2]) : "HTTP/1.1"
        let connection = header("connection", in: head)?.lowercased()
        let keepAlive = connection.map { $0.contains("keep-alive") } ?? version.hasSuffix("1.1")
        return HTTPRequest(
            method: String(parts[0]), path: String(parts[1]), body: body, keepAlive: keepAlive)
    }

    private static func header(_ name: String, in head: Data) -> String? {
        for line in head.split(separator: UInt8(ascii: "\n")) {
            // `\r` is part of every header line: trimming only spaces leaves it in the value, and
            // `Int("195\r")` is nil, which turned every request with a body into a 400.
            guard
                let text = String(bytes: line, encoding: .utf8)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
            else { continue }
            let pieces = text.split(separator: ":", maxSplits: 1)
            guard pieces.count == 2, pieces[0].lowercased() == name else { continue }
            return pieces[1].trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return nil
    }
}
