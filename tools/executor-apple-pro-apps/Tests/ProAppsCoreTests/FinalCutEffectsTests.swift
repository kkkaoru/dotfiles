import Foundation
import Testing

@testable import ProAppsCore

struct FinalCutEffectsTests {
  static let work = URL(fileURLWithPath: "/Users/u/work dir")
  static let gaussian = ".../Effects.localized/Blur.localized/Gaussian.localized/Gaussian.moef"

  static func keyframes(_ pairs: [(Double, String)], curve: FCPKeyframeCurve? = nil)
    -> FCPAnimatedValue
  {
    FCPAnimatedValue(keyframes: pairs.map { FCPKeyframe(seconds: $0.0, value: $0.1, curve: curve) })
  }

  @Test func buildsACarrierWithAdjustmentsEffectsAndKeyframes() throws {
    let spec = FCPCarrierSpec(
      durationSeconds: 90, opacity: Self.keyframes([(60, "1"), (70, "0")]),
      position: Self.keyframes([(30, "0 0"), (40, "50 0")], curve: .smooth),
      scale: FCPAnimatedValue(value: "1.5 1.5"), rotation: FCPAnimatedValue(value: "15"),
      effects: [
        FCPEffectSpec(
          uid: Self.gaussian, name: "Gaussian & <Blur>",
          parameters: [
            FCPEffectParameter(
              name: "Amount", key: "9999/986883370/100/986883376/2/100",
              animation: Self.keyframes([(0, "0"), (4, "1")])),
            FCPEffectParameter(
              name: "Horizontal", key: "9999/986883354/986883358/3/986883361/2",
              animation: FCPAnimatedValue(value: "37")),
          ]),
        FCPEffectSpec(uid: "FFColorBoard", name: "Board"),
      ])
    let document = try FCPCarrierDocument.make(spec, workDirectory: Self.work, project: "carrier-1")
    #expect(document.library == "Claude-Effect-Carriers")
    #expect(document.event == "Carriers")
    #expect(document.project == "carrier-1")
    let xml = document.xml
    #expect(xml.contains(#"value="file:///Users/u/work%20dir/Claude-Effect-Carriers.fcpbundle"#))
    #expect(xml.contains(#"<sequence format="f1" duration="2700/30s""#))
    #expect(
      xml.contains(
        #"<adjust-transform scale="1.5 1.5" rotation="15"><param name="position"><keyframeAnimation><keyframe time="900/30s" value="0 0" curve="smooth"/><keyframe time="1200/30s" value="50 0" curve="smooth"/></keyframeAnimation></param></adjust-transform>"#
      ))
    #expect(
      xml.contains(
        #"<adjust-blend><param name="amount"><keyframeAnimation><keyframe time="1800/30s" value="1" curve="linear"/><keyframe time="2100/30s" value="0" curve="linear"/></keyframeAnimation></param></adjust-blend>"#
      ))
    #expect(xml.contains(#"name="Gaussian &amp; &lt;Blur&gt;" uid=".../Effects.localized"#))
    #expect(
      xml.contains(
        #"<param name="Horizontal" key="9999/986883354/986883358/3/986883361/2" value="37"/>"#))
    #expect(xml.contains(#"<filter-video ref="e2" name="Board"></filter-video>"#))
  }

  @Test func acceptsAnOpacityOnlyCarrierAtNTSCRates() throws {
    let spec = FCPCarrierSpec(
      durationSeconds: 2, frameDuration: "1001/30000s", opacity: FCPAnimatedValue(value: "0.5"))
    let xml = try FCPCarrierDocument.make(spec, workDirectory: Self.work, project: "p").xml
    #expect(xml.contains(#"<adjust-blend amount="0.5"></adjust-blend>"#))
    #expect(xml.contains(#"duration="60060/30000s""#))
  }

  @Test(arguments: [
    FCPCarrierSpec(durationSeconds: 0, opacity: FCPAnimatedValue(value: "1")),
    FCPCarrierSpec(durationSeconds: 7 * 3600, opacity: FCPAnimatedValue(value: "1")),
    FCPCarrierSpec(durationSeconds: 1),
    FCPCarrierSpec(
      durationSeconds: 1, frameDuration: "30fps", opacity: FCPAnimatedValue(value: "1")),
    FCPCarrierSpec(
      durationSeconds: 1, frameDuration: "1/1000s", opacity: FCPAnimatedValue(value: "1")),
    FCPCarrierSpec(durationSeconds: 1, opacity: FCPAnimatedValue(value: "1.5")),
    FCPCarrierSpec(durationSeconds: 1, opacity: FCPAnimatedValue(value: "half")),
    FCPCarrierSpec(durationSeconds: 1, opacity: FCPAnimatedValue()),
    FCPCarrierSpec(
      durationSeconds: 1, opacity: FCPAnimatedValue(value: "1", keyframes: [])),
    FCPCarrierSpec(durationSeconds: 1, opacity: FCPAnimatedValue(keyframes: [])),
    FCPCarrierSpec(durationSeconds: 1, position: FCPAnimatedValue(value: "1")),
    FCPCarrierSpec(durationSeconds: 1, rotation: FCPAnimatedValue(value: "1 2")),
    FCPCarrierSpec(durationSeconds: 1, opacity: keyframes([(0.5, "1"), (0.2, "0")])),
    FCPCarrierSpec(durationSeconds: 1, opacity: keyframes([(-1, "1")])),
    FCPCarrierSpec(durationSeconds: 1, opacity: keyframes([(2, "1")])),
    FCPCarrierSpec(durationSeconds: 1, opacity: keyframes([(.nan, "1")])),
    FCPCarrierSpec(durationSeconds: 1, effects: [FCPEffectSpec(uid: "/abs/x.moef", name: "x")]),
    FCPCarrierSpec(durationSeconds: 1, effects: [FCPEffectSpec(uid: "", name: "x")]),
    FCPCarrierSpec(
      durationSeconds: 1,
      effects: [
        FCPEffectSpec(
          uid: gaussian, name: "g",
          parameters: [
            FCPEffectParameter(name: "a", key: "1/2", animation: FCPAnimatedValue(value: "1"))
          ])
      ]),
    FCPCarrierSpec(
      durationSeconds: 1, effects: [FCPEffectSpec(uid: gaussian, name: "bad\u{0007}")]),
    FCPCarrierSpec(
      durationSeconds: 1,
      effects: Array(repeating: FCPEffectSpec(uid: gaussian, name: "g"), count: 17)),
  ])
  func rejectsInvalidCarriers(_ spec: FCPCarrierSpec) {
    #expect(throws: ProAppsError.self) {
      try FCPCarrierDocument.make(spec, workDirectory: Self.work, project: "p")
    }
  }

  @Test func rejectsTooManyKeyframesAndInvalidProjectNames() {
    let many = (0...1000).map { FCPKeyframe(seconds: Double($0) / 30, value: "1") }
    #expect(throws: ProAppsError.invalid("Use 1–1000 keyframes per parameter")) {
      try FCPCarrierDocument.make(
        FCPCarrierSpec(durationSeconds: 60, opacity: FCPAnimatedValue(keyframes: many)),
        workDirectory: Self.work, project: "p")
    }
    #expect(throws: ProAppsError.self) {
      try FCPCarrierDocument.make(
        FCPCarrierSpec(durationSeconds: 1, opacity: FCPAnimatedValue(value: "1")),
        workDirectory: Self.work, project: "")
    }
  }

  @Test(arguments: [
    ("37", "37.0", true), ("33.333", "33.3", true), ("33.4", "33.3", false), ("0", "0", true),
    ("1.5", "1.50", true), ("standard", "standard", true), ("a", "b", false), ("1", "x", false),
  ])
  func comparesInspectorValuesNumerically(_ requested: String, _ observed: String, _ same: Bool) {
    #expect(fcpValuesMatch(requested, observed) == same)
  }

  // MARK: Catalog

  /// A synthetic app bundle with two template roots, one duplicate UID, a
  /// localized and an unlocalized template, and a malformed strings table.
  func bundle() throws -> URL {
    let app = FileManager.default.temporaryDirectory.appendingPathComponent(
      "catalog-\(UUID().uuidString)/Final Cut Pro.app")
    let resources = app.appendingPathComponent(FCPEffectCatalog.resources)
    func template(_ root: String, _ path: String) throws -> URL {
      let file = resources.appendingPathComponent("\(root)/Effects.localized/\(path)")
      try FileManager.default.createDirectory(
        at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
      try Data("<ozml/>".utf8).write(to: file)
      return file
    }
    func strings(_ folder: URL, _ table: [String: String]) throws {
      let localized = folder.appendingPathComponent(".localized")
      try FileManager.default.createDirectory(at: localized, withIntermediateDirectories: true)
      try PropertyListEncoder().encode(table).write(
        to: localized.appendingPathComponent("ja.strings"))
    }
    let gaussian = try template(
      "PETemplates.localized", "Blur.localized/Gaussian.localized/Gaussian.moef")
    try strings(gaussian.deletingLastPathComponent(), ["Gaussian": "ガウス"])
    try strings(
      resources.appendingPathComponent("PETemplates.localized/Effects.localized/Blur.localized"),
      ["Blur": "ブラー"])
    _ = try template("Templates.localized", "Blur.localized/Gaussian.localized/Gaussian.moef")
    let glow = try template("Templates.localized", "Light.localized/Glow.localized/Glow.moef")
    let broken = glow.deletingLastPathComponent().appendingPathComponent(".localized")
    try FileManager.default.createDirectory(at: broken, withIntermediateDirectories: true)
    try Data("not a plist".utf8).write(to: broken.appendingPathComponent("ja.strings"))
    _ = try template("Templates.localized", "Top.moef")
    try strings(
      resources.appendingPathComponent("Templates.localized/Effects.localized/Light.localized"),
      ["Other": "その他"])
    try Data("x".utf8).write(
      to: resources.appendingPathComponent("Templates.localized/Effects.localized/readme.txt"))
    try FileManager.default.createDirectory(
      at: resources.appendingPathComponent("iOSTemplates.localized"),
      withIntermediateDirectories: true)
    try FileManager.default.createDirectory(
      at: resources.appendingPathComponent("Base.lproj"), withIntermediateDirectories: true)
    return app
  }

  @Test func catalogsLocalizedTemplatesOnce() throws {
    let app = try bundle()
    let all = try FCPEffectCatalog.scan(application: app, language: "ja", query: nil)
    #expect(
      all == [
        FCPEffectTemplate(
          uid: ".../Effects.localized/Blur.localized/Gaussian.localized/Gaussian.moef",
          name: "Gaussian", localizedName: "ガウス", category: "ブラー"),
        FCPEffectTemplate(
          uid: ".../Effects.localized/Light.localized/Glow.localized/Glow.moef", name: "Glow",
          localizedName: "Glow", category: "Light"),
      ])
    #expect(try FCPEffectCatalog.scan(application: app, language: "ja", query: "ブラー").count == 1)
    #expect(try FCPEffectCatalog.scan(application: app, language: "ja", query: "").count == 2)
    #expect(try FCPEffectCatalog.scan(application: app, language: "ja", query: "none").isEmpty)
    #expect(
      try FCPEffectCatalog.scan(application: app, language: "en", query: nil).first?.localizedName
        == "Gaussian")
    try FileManager.default.removeItem(at: app.deletingLastPathComponent())
  }

  /// A synthetic Motion template: a rig widget, a filter on a layer's image node,
  /// a grouped text node, a target without an object and one with a bad channel.
  static let template = """
    <?xml version="1.0" encoding="UTF-8"?>
    <ozml>
      <factory id="2"><description>Rig</description></factory>
      <factory id="4"><description>Widget</description></factory>
      <factory id="5"><description>Image</description></factory>
      <factory id="7"><description>Text</description></factory>
      <factory id="9"/>
      <scene>
        <publishSettings>
          <target object="11" channel="./2/100" name="Amount"/>
          <target object="22" channel="./2" name="Horizontal"/>
          <target object="33" channel="./2/369" name="Text"/>
          <target object="99" channel="./1" name="Missing"/>
          <target object="11" channel="2" name="Bad Channel"/>
          <target object="41" channel="./1" name="Plain"/>
          <target object="51" channel="./2" name="Anonymous Parent"/>
        </publishSettings>
        <scenenode name="Rig" id="10" factoryID="2"><scenenode name="Amount" id="11" factoryID="4"/></scenenode>
        <layer name="Group" id="20">
          <scenenode name="Source" id="21" factoryID="5"><filter name="Blur" id="22"/></scenenode>
          <group name="Texts" id="30"><scenenode name="Label" id="33" factoryID="7"/></group>
          <scenenode name="No Factory" id="41"/>
          <group name="Unnamed"><scenenode name="Undescribed" id="51" factoryID="9"/></group>
        </layer>
      </scene>
    </ozml>
    """

  @Test func derivesParameterKeysFromTheTemplateHierarchy() throws {
    let parameters = try FCPEffectCatalog.parameters(template: Data(Self.template.utf8))
    #expect(
      parameters == [
        FCPEffectTemplateParameter(
          name: "Amount", key: "9999/10/100/11/2/100", structure: "Rig > Widget", verified: true),
        FCPEffectTemplateParameter(
          name: "Horizontal", key: "9999/20/21/3/22/2", structure: "layer > Image > filter",
          verified: true),
        FCPEffectTemplateParameter(
          name: "Text", key: "9999/20/30/33/2/369", structure: "layer > group > Text",
          verified: true),
        FCPEffectTemplateParameter(
          name: "Plain", key: "9999/20/41/1", structure: "layer > -", verified: false),
        FCPEffectTemplateParameter(
          name: "Anonymous Parent", key: "9999/20//51/2", structure: "layer > group > ",
          verified: false),
      ])
    #expect(throws: ProAppsError.unavailable("The effect template is not readable XML")) {
      try FCPEffectCatalog.parameters(template: Data("<ozml".utf8))
    }
  }

  @Test func resolvesInstalledTemplatesForParameters() throws {
    let app = try bundle()
    let template = app.appendingPathComponent(
      FCPEffectCatalog.resources
        + "/PETemplates.localized/Effects.localized/Blur.localized/Gaussian.localized/Gaussian.moef"
    )
    try Data(Self.template.utf8).write(to: template)
    let uid = ".../Effects.localized/Blur.localized/Gaussian.localized/Gaussian.moef"
    #expect(try FCPEffectCatalog.parameters(application: app, uid: uid).count == 5)
    for invalid in ["FxPlug:x", ".../Effects.localized/../x.moef", ".../Effects.localized/a.motn"] {
      #expect(throws: ProAppsError.self) {
        try FCPEffectCatalog.parameters(application: app, uid: invalid)
      }
    }
    #expect(throws: ProAppsError.unavailable("The effect template is not installed")) {
      try FCPEffectCatalog.parameters(
        application: app, uid: ".../Effects.localized/Absent.localized/A.moef")
    }
    try FileManager.default.removeItem(at: app.deletingLastPathComponent())
  }

  /// Synthetic built-in FxPlug list: offered, hidden, obsolete, OSC and placeholder entries.
  func writeFilters(_ app: URL, names: Data? = nil, list: Data? = nil) throws {
    let contents = app.appendingPathComponent(FCPEffectCatalog.filtersBundle)
    try FileManager.default.createDirectory(
      at: contents.appendingPathComponent("Resources/ja.lproj"), withIntermediateDirectories: true)
    struct Entry: Encodable {
      let uuid: String?
      let displayName: String?
      let className: String?
      let obsolete: String?
      let finalCutSimplifiedList: Bool?
    }
    struct List: Encodable {
      let entries: [Entry]
      enum CodingKeys: String, CodingKey { case entries = "ProPlugPlugInList" }
    }
    let shadow = "9C13F991-BC99-4DC8-B150-381D7CCE183B"
    let entries = [
      Entry(
        uuid: shadow, displayName: "DropShadow::Filter Name", className: "PAEDropShadow",
        obsolete: nil, finalCutSimplifiedList: true),
      Entry(
        uuid: shadow, displayName: "DropShadow::Filter Name", className: "PAEDropShadow",
        obsolete: nil, finalCutSimplifiedList: true),
      Entry(
        uuid: "11111111-1111-1111-1111-111111111111", displayName: "Raw", className: "PAERaw",
        obsolete: nil, finalCutSimplifiedList: true),
      Entry(
        uuid: "22222222-2222-2222-2222-222222222222", displayName: "x", className: "PAEColorOSC",
        obsolete: nil, finalCutSimplifiedList: true),
      Entry(
        uuid: "33333333-3333-3333-3333-333333333333", displayName: "x", className: "PAEOld",
        obsolete: "1", finalCutSimplifiedList: true),
      Entry(
        uuid: "44444444-4444-4444-4444-444444444444", displayName: "x", className: "PAEHidden",
        obsolete: nil, finalCutSimplifiedList: nil),
      Entry(
        uuid: nil, displayName: nil, className: nil, obsolete: nil, finalCutSimplifiedList: true),
    ]
    let encoder = PropertyListEncoder()
    encoder.outputFormat = .binary
    try (list ?? encoder.encode(List(entries: entries))).write(
      to: contents.appendingPathComponent("Info.plist"))
    try (names ?? encoder.encode(["DropShadow::Filter Name": "ドロップシャドウ"])).write(
      to: contents.appendingPathComponent("Resources/ja.lproj/Localizable.strings"))
  }

  @Test func catalogsOfferedFxPlugFilters() throws {
    let app = try bundle()
    try writeFilters(app)
    let fx = try FCPEffectCatalog.scan(application: app, language: "ja", query: "FxPlug")
    #expect(
      fx == [
        FCPEffectTemplate(
          uid: "FxPlug:11111111-1111-1111-1111-111111111111", name: "PAERaw",
          localizedName: "PAERaw",
          category: "FxPlug"),
        FCPEffectTemplate(
          uid: "FxPlug:9C13F991-BC99-4DC8-B150-381D7CCE183B", name: "PAEDropShadow",
          localizedName: "ドロップシャドウ", category: "FxPlug"),
      ])
    #expect(
      try FCPEffectCatalog.scan(application: app, language: "en", query: "PAEDrop").count == 1)
    try writeFilters(app, names: Data("bad".utf8))
    #expect(throws: ProAppsError.unavailable("The FxPlug name table is malformed")) {
      try FCPEffectCatalog.fxPlugEffects(application: app, language: "ja")
    }
    try writeFilters(app, list: Data("bad".utf8))
    #expect(throws: ProAppsError.unavailable("The FxPlug filter list is malformed")) {
      try FCPEffectCatalog.fxPlugEffects(application: app, language: "ja")
    }
    try FileManager.default.removeItem(at: app.deletingLastPathComponent())
  }

  @Test func carriersAcceptOnlyWellFormedFxPlugIdentifiers() throws {
    let valid = FCPCarrierSpec(
      durationSeconds: 1,
      effects: [FCPEffectSpec(uid: "FxPlug:9C13F991-BC99-4DC8-B150-381D7CCE183B", name: "S")])
    #expect(
      try FCPCarrierDocument.make(valid, workDirectory: Self.work, project: "p").xml.contains(
        #"uid="FxPlug:9C13F991-BC99-4DC8-B150-381D7CCE183B""#))
    let invalid = FCPCarrierSpec(
      durationSeconds: 1, effects: [FCPEffectSpec(uid: "FxPlug:not-a-uuid", name: "S")])
    #expect(throws: ProAppsError.self) {
      try FCPCarrierDocument.make(invalid, workDirectory: Self.work, project: "p")
    }
  }

  @Test func catalogRequiresTheTemplateResources() {
    #expect(throws: (any Error).self) {
      try FCPEffectCatalog.scan(
        application: URL(fileURLWithPath: "/nonexistent.app"), language: "ja", query: nil)
    }
  }
}
