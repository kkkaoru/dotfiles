/// Typed read-only projection of IOPMrootDomain's clamshell state.
internal struct LidRecord: Decodable {
  private enum CodingKeys: String, CodingKey {
    case isClosed = "AppleClamshellState"
  }

  internal let isClosed: Bool?
}
