import Foundation

internal enum ProbeOutputParser {
  internal struct HTTPResult: Sendable {
    internal let statusCode: Int?
    internal let seconds: Double?
  }

  internal struct HTTPCheck: Sendable {
    internal let command: CommandResult
    internal let response: HTTPResult
  }

  private static let minimumHTTPFieldCount = 2

  internal static func ping(_ output: String) -> PingResult {
    var average: Double?
    var loss: Double?

    for line in output.split(whereSeparator: \.isNewline) {
      let text = String(line)
      if text.contains("round-trip") {
        let values = text.split(separator: "=").dropFirst().first?.split(separator: "/")
        if let averageValue = values?.dropFirst().first {
          average = Double(averageValue.trimmingCharacters(in: .whitespaces))
        }
      }

      if text.contains("packets transmitted") {
        let token = text.split { character in
          character == " " || character == "\t"
        }
        .first { token in token.hasSuffix("%") }
        loss = token.flatMap { Double($0.dropLast()) }
      }
    }

    return PingResult(averageMilliseconds: average, packetLossPercent: loss)
  }

  internal static func http(_ output: String) -> HTTPResult {
    let fields = output.split { character in
      character == " " || character == "\n"
    }
    guard fields.count >= Self.minimumHTTPFieldCount else {
      return HTTPResult(statusCode: nil, seconds: nil)
    }
    return HTTPResult(statusCode: Int(fields[0]), seconds: Double(fields[1]))
  }
}
