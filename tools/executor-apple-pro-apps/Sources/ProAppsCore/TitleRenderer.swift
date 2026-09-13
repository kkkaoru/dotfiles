import CoreGraphics
import Foundation
import SwiftUI

/// SwiftUI's offscreen renderer requires MainActor. Only bounded text rendering
/// runs there; native video export and file I/O remain on their own executors.
/// No window, screenshot, attributed-string dictionary or pixel pointer is used.
enum TitleRenderer {
  @MainActor
  static func render(_ title: EditTitle, canvas: EditVideoSettings) throws -> CGImage {
    try Task.checkCancellation()
    try EditPlan.validate(title, canvas: canvas)
    let view = Text(verbatim: title.text)
      .font(.system(size: title.fontSize, weight: .bold))
      .foregroundStyle(.white)
      .fixedSize()
    let renderer = ImageRenderer(content: view)
    renderer.scale = 1
    renderer.isOpaque = false
    guard let image = renderer.cgImage else {
      throw ProAppsError.unavailable("Offscreen text rendering is unavailable")
    }
    guard image.width > 0, image.height > 0,
      Double(image.width) + title.x <= Double(canvas.width),
      Double(image.height) + title.y <= Double(canvas.height)
    else {
      throw ProAppsError.invalid("Rendered title exceeds the canvas; shorten it or reduce its size")
    }
    return image
  }
}
