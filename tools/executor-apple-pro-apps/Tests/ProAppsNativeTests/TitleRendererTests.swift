import CoreImage
import Foundation
import Testing

@testable import ProAppsCore

struct TitleRendererTests {
  @Test @MainActor func offscreenTextHasVisibleGlyphsWithoutTruncation() throws {
    let title = EditTitle(text: "TEST", x: 8, y: 8, fontSize: 32)
    let canvas = EditVideoSettings(
      width: 320, height: 240, frameRate: 30, resizeMode: .fit, titles: [title])
    let image = try TitleRenderer.render(title, canvas: canvas)
    #expect(image.width > 32 && image.width < 312)
    #expect(image.height > 16 && image.height < 232)
    let pixels = CIImage(cgImage: image)
    let opaque = pixels.composited(over: CIImage(color: .black).cropped(to: pixels.extent))
    let color = try FrameProbe.average(opaque, region: pixels.extent, context: CIContext())
    #expect(color.red > 0.02)
    #expect(abs(color.red - color.green) < 0.01)
    let saved = try JSONDecoder().decode(EditVideoSettings.self, from: JSONEncoder().encode(canvas))
    #expect(saved.titles?.first?.text == "TEST")
  }

  @Test @MainActor func oversizedTextIsRefusedInsteadOfClipped() {
    #expect(throws: (any Error).self) {
      try TitleRenderer.render(
        .init(text: "Too wide", x: 0, y: 0, fontSize: 128),
        canvas: .init(width: 32, height: 32, frameRate: 30, resizeMode: .fit))
    }
  }
}
