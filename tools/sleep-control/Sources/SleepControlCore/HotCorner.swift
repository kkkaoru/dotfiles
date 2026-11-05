import CoreGraphics

/// A screen corner, with the Dock's preference-key prefix as its raw value.
internal enum HotCorner: String, CaseIterable {
  case bottomLeft = "bl"
  case bottomRight = "br"
  case topLeft = "tl"
  case topRight = "tr"

  internal func point(of screen: CGRect) -> CGPoint {
    switch self {
    case .topLeft:
      CGPoint(x: screen.minX, y: screen.maxY)

    case .topRight:
      CGPoint(x: screen.maxX, y: screen.maxY)

    case .bottomLeft:
      CGPoint(x: screen.minX, y: screen.minY)

    case .bottomRight:
      CGPoint(x: screen.maxX, y: screen.minY)
    }
  }
}
