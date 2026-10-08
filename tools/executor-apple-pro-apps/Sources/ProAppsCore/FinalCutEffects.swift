import Foundation

/// Keyframe interpolation curve. Final Cut Pro's FCPXML default is `smooth`;
/// carriers default to `linear` so values between keyframes are predictable.
public enum FCPKeyframeCurve: String, Codable, CaseIterable, Sendable {
  case linear, smooth
}

/// One keyframe at a clip-relative time in seconds (rounded to whole frames).
public struct FCPKeyframe: Codable, Equatable, Sendable {
  public let seconds: Double
  public let value: String
  public let curve: FCPKeyframeCurve?

  public init(seconds: Double, value: String, curve: FCPKeyframeCurve? = nil) {
    self.seconds = seconds
    self.value = value
    self.curve = curve
  }
}

/// A parameter that is either constant (`value`) or animated (`keyframes`).
public struct FCPAnimatedValue: Codable, Equatable, Sendable {
  public let value: String?
  public let keyframes: [FCPKeyframe]?

  public init(value: String? = nil, keyframes: [FCPKeyframe]? = nil) {
    self.value = value
    self.keyframes = keyframes
  }
}

/// One effect parameter addressed by Final Cut Pro's FCPXML key. Keys depend on
/// the template's internal object hierarchy; read them from Final Cut Pro's own
/// XML export (`fcp_xml_export`) rather than guessing.
public struct FCPEffectParameter: Codable, Equatable, Sendable {
  public let name: String
  public let key: String
  public let animation: FCPAnimatedValue

  public init(name: String, key: String, animation: FCPAnimatedValue) {
    self.name = name
    self.key = key
    self.animation = animation
  }
}

/// One video effect by template UID (for example from `fcp_effect_catalog`).
public struct FCPEffectSpec: Codable, Equatable, Sendable {
  public let uid: String
  public let name: String
  public let parameters: [FCPEffectParameter]

  public init(uid: String, name: String, parameters: [FCPEffectParameter] = []) {
    self.uid = uid
    self.name = name
    self.parameters = parameters
  }
}

/// Built-in clip attributes and effects carried by a generated FCPXML clip.
/// Opacity is 0–1; position/anchor are "x y" in FCPXML units (percent of frame
/// height); scale is "x y" factors; rotation is degrees.
public struct FCPCarrierSpec: Codable, Equatable, Sendable {
  public let durationSeconds: Double
  public let frameDuration: String
  public let opacity: FCPAnimatedValue?
  public let position: FCPAnimatedValue?
  public let scale: FCPAnimatedValue?
  public let rotation: FCPAnimatedValue?
  public let anchor: FCPAnimatedValue?
  public let effects: [FCPEffectSpec]

  public init(
    durationSeconds: Double, frameDuration: String = "1/30s", opacity: FCPAnimatedValue? = nil,
    position: FCPAnimatedValue? = nil, scale: FCPAnimatedValue? = nil,
    rotation: FCPAnimatedValue? = nil, anchor: FCPAnimatedValue? = nil,
    effects: [FCPEffectSpec] = []
  ) {
    self.durationSeconds = durationSeconds
    self.frameDuration = frameDuration
    self.opacity = opacity
    self.position = position
    self.scale = scale
    self.rotation = rotation
    self.anchor = anchor
    self.effects = effects
  }
}

/// Paste the carrier's effects and attributes onto clips of an open project.
public struct FCPPasteRequest: Codable, Equatable, Sendable {
  public let library: String
  public let event: String
  public let project: String
  public let targets: [FCPClipReference]
  public let carrier: FCPCarrierSpec
  public let workDirectory: String
  public let mode: FCPPasteMode
  public let closeCarrierLibrary: Bool

  public init(
    library: String, event: String, project: String, targets: [FCPClipReference],
    carrier: FCPCarrierSpec, workDirectory: String, mode: FCPPasteMode = .merge,
    closeCarrierLibrary: Bool = true
  ) {
    self.library = library
    self.event = event
    self.project = project
    self.targets = targets
    self.carrier = carrier
    self.workDirectory = workDirectory
    self.mode = mode
    self.closeCarrierLibrary = closeCarrierLibrary
  }
}

/// Validated carrier FCPXML for one paste.
public struct FCPCarrierDocument: Equatable, Sendable {
  public let xml: String
  public let library: String
  public let event: String
  public let project: String

  /// Disposable library (inside the work directory) that receives every carrier.
  public static let libraryName = "Claude-Effect-Carriers"
  public static let eventName = "Carriers"
  static let generatorUID = ".../Generators.localized/Solids.localized/Custom.localized/Custom.motn"
  static let maximumSeconds = 6 * 3600.0
  static let maximumText = 512
  static let maximumKeyframes = 1_000
  static let maximumEffects = 16

  /// Build the carrier for `spec`. The clip is a solid-color generator: pasting
  /// effects copies its effects and attributes, keyframes stay clip-relative.
  public static func make(_ spec: FCPCarrierSpec, workDirectory: URL, project: String) throws
    -> FCPCarrierDocument
  {
    let rate = try FrameRate(spec.frameDuration)
    let frames = rate.frames(spec.durationSeconds)
    guard frames > 0, spec.durationSeconds <= maximumSeconds else {
      throw ProAppsError.invalid("durationSeconds must cover at least one frame and ≤ 6 hours")
    }
    let attributes = [spec.opacity, spec.position, spec.scale, spec.rotation, spec.anchor]
    guard spec.effects.count <= maximumEffects,
      !spec.effects.isEmpty || attributes.contains(where: { $0 != nil })
    else { throw ProAppsError.invalid("A carrier needs 1–16 effects or at least one attribute") }
    let library = workDirectory.appendingPathComponent(libraryName + ".fcpbundle")
    let location = escape(library.absoluteString)
    var resources = [
      #"<format id="f1" frameDuration="\#(rate.text)" width="1920" height="1080"/>"#,
      #"<effect id="g1" name="Custom" uid="\#(generatorUID)"/>"#,
    ]
    var filters: [String] = []
    for (offset, effect) in spec.effects.enumerated() {
      guard
        effect.uid.hasPrefix(".../") || effect.uid.hasPrefix("~/") || isIdentifier(effect.uid)
          || isFxPlug(effect.uid)
      else { throw ProAppsError.invalid("Effect uid must be a template path or identifier") }
      let id = "e\(offset + 1)"
      resources.append(
        #"<effect id="\#(id)" name="\#(try text(effect.name))" uid="\#(try text(effect.uid))"/>"#)
      let parameters = try effect.parameters.map { parameter in
        guard parameter.key.hasPrefix("9999/") else {
          throw ProAppsError.invalid("Effect parameter keys come from Final Cut Pro's XML export")
        }
        return try param(
          parameter.name, key: parameter.key, parameter.animation, rate: rate, frames: frames,
          check: nil)
      }
      filters.append(
        #"<filter-video ref="\#(id)" name="\#(try text(effect.name))">"# + parameters.joined()
          + "</filter-video>")
    }
    var adjustments = ""
    let transform: [(String, FCPAnimatedValue?, Int)] = [
      ("position", spec.position, 2), ("scale", spec.scale, 2), ("rotation", spec.rotation, 1),
      ("anchor", spec.anchor, 2),
    ]
    if transform.contains(where: { $0.1 != nil }) {
      adjustments += try adjustment("adjust-transform", transform, rate: rate, frames: frames)
    }
    if spec.opacity != nil {
      adjustments += try adjustment(
        "adjust-blend", [("amount", spec.opacity, 0)], rate: rate, frames: frames)
    }
    let duration = rate.time(frames)
    let name = try text(project)
    let xml = """
      <?xml version="1.0" encoding="UTF-8"?>
      <!DOCTYPE fcpxml>
      <fcpxml version="1.13">
        <import-options>
          <option key="library location" value="\(location)"/>
          <option key="suppress warnings" value="1"/>
        </import-options>
        <resources>\(resources.joined())</resources>
        <library location="\(location)">
          <event name="\(eventName)">
            <project name="\(name)">
              <sequence format="f1" duration="\(duration)" tcStart="0s" tcFormat="NDF">
                <spine><video ref="g1" name="\(name)" offset="0s" start="0s" duration="\(duration)">\(adjustments)\(filters.joined())</video></spine>
              </sequence>
            </project>
          </event>
        </library>
      </fcpxml>

      """
    return FCPCarrierDocument(xml: xml, library: libraryName, event: eventName, project: project)
  }

  /// A built-in adjustment: constant values become attributes and animated values
  /// become keyframed params (the forms Final Cut Pro itself exports).
  /// `count` is the number of space-separated numbers; 0 means an opacity in 0–1.
  static func adjustment(
    _ element: String, _ values: [(String, FCPAnimatedValue?, Int)], rate: FrameRate, frames: Int
  ) throws -> String {
    var attributes = ""
    var params = ""
    for (name, value, count) in values {
      guard let value else { continue }
      let check: (String) throws -> Void = {
        count == 0 ? try unit($0) : try numbers($0, count: count)
      }
      if let constant = value.value, value.keyframes == nil {
        try check(constant)
        attributes += #" \#(name)="\#(try text(constant))""#
      } else {
        params += try param(name, key: nil, value, rate: rate, frames: frames, check: check)
      }
    }
    return "<\(element)\(attributes)>\(params)</\(element)>"
  }

  static func param(
    _ name: String, key: String?, _ animation: FCPAnimatedValue, rate: FrameRate, frames: Int,
    check: ((String) throws -> Void)?
  ) throws -> String {
    let keyAttribute = try key.map { #" key="\#(try text($0))""# } ?? ""
    switch (animation.value, animation.keyframes) {
    case (.some(let value), .none):
      try check?(value)
      return #"<param name="\#(try text(name))"\#(keyAttribute) value="\#(try text(value))"/>"#
    case (.none, .some(let keyframes)):
      guard (1...maximumKeyframes).contains(keyframes.count) else {
        throw ProAppsError.invalid("Use 1–1000 keyframes per parameter")
      }
      var previous = -1
      let items = try keyframes.map { keyframe -> String in
        let frame = rate.frames(keyframe.seconds)
        guard keyframe.seconds >= 0, frame <= frames, frame > previous else {
          throw ProAppsError.invalid(
            "Keyframes must be inside the carrier, on distinct frames, in time order")
        }
        previous = frame
        try check?(keyframe.value)
        let curve = (keyframe.curve ?? .linear).rawValue
        return
          #"<keyframe time="\#(rate.time(frame))" value="\#(try text(keyframe.value))" curve="\#(curve)"/>"#
      }
      return #"<param name="\#(try text(name))"\#(keyAttribute)><keyframeAnimation>"#
        + items.joined() + "</keyframeAnimation></param>"
    default:
      throw ProAppsError.invalid("Give each parameter exactly one of value or keyframes")
    }
  }

  static func numbers(_ value: String, count: Int) throws {
    let parts = value.split(separator: " ")
    guard parts.count == count, parts.allSatisfy({ Double($0)?.isFinite == true }) else {
      throw ProAppsError.invalid("Expected \(count) space-separated numbers: \(value)")
    }
  }

  static func unit(_ value: String) throws {
    guard let number = Double(value), (0...1).contains(number) else {
      throw ProAppsError.invalid("Opacity is a number from 0 to 1")
    }
  }

  /// Built-in FxPlug filters are referenced as `FxPlug:<plug-in UUID>`.
  static func isFxPlug(_ text: String) -> Bool {
    text.hasPrefix(fxPlugPrefix)
      && UUID(uuidString: String(text.dropFirst(fxPlugPrefix.count))) != nil
  }

  static let fxPlugPrefix = "FxPlug:"

  static func isIdentifier(_ text: String) -> Bool {
    !text.isEmpty && text.allSatisfy { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == ".") }
  }

  /// Escape bounded caller text for an XML attribute; control characters are refused.
  static func text(_ value: String) throws -> String {
    guard (1...maximumText).contains(value.count),
      !value.unicodeScalars.contains(where: { $0.properties.generalCategory == .control })
    else { throw ProAppsError.invalid("Text must be 1–512 characters without control codes") }
    return escape(value)
  }

  static func escape(_ value: String) -> String {
    value.replacingOccurrences(of: "&", with: "&amp;").replacingOccurrences(of: "<", with: "&lt;")
      .replacingOccurrences(of: ">", with: "&gt;").replacingOccurrences(of: "\"", with: "&quot;")
      .replacingOccurrences(of: "'", with: "&apos;")
  }
}

/// A rational frame duration such as `1/30s` or `1001/30000s`.
struct FrameRate: Equatable {
  let numerator: Int
  let denominator: Int
  static let maximumDenominator = 1_000_000

  init(_ text: String) throws {
    let parts = text.hasSuffix("s") ? text.dropLast().split(separator: "/") : []
    guard parts.count == 2, let numerator = Int(parts[0]), let denominator = Int(parts[1]),
      numerator > 0, (1...Self.maximumDenominator).contains(denominator),
      denominator / numerator <= 240
    else { throw ProAppsError.invalid("frameDuration must look like 1/30s or 1001/30000s") }
    self.numerator = numerator
    self.denominator = denominator
  }

  var text: String { "\(numerator)/\(denominator)s" }

  func frames(_ seconds: Double) -> Int {
    guard seconds.isFinite else { return -1 }
    return Int((seconds * Double(denominator) / Double(numerator)).rounded())
  }

  func time(_ frames: Int) -> String { "\(frames * numerator)/\(denominator)s" }
}

/// One installed Final Cut Pro effect template.
public struct FCPEffectTemplate: Codable, Equatable, Sendable {
  public let uid: String
  public let name: String
  public let localizedName: String
  public let category: String
}

/// Enumerates Motion effect templates inside a Final Cut Pro bundle. The FCPXML
/// UID is the template path below `Effects.localized`, prefixed with `.../`.
public enum FCPEffectCatalog {
  static let resources = "Contents/PlugIns/MediaProviders/MotionEffect.fxp/Contents/Resources"
  static let effectsFolder = "Effects.localized"
  static let maximumResults = 500

  public static func scan(application: URL, language: String, query: String?) throws
    -> [FCPEffectTemplate]
  {
    let root = application.appendingPathComponent(resources)
    let templateRoots = try FileManager.default.contentsOfDirectory(
      at: root, includingPropertiesForKeys: nil
    ).filter { $0.lastPathComponent.hasSuffix("Templates.localized") }
    var seen = Set<String>()
    var found: [FCPEffectTemplate] = []
    for templates in templateRoots.sorted(by: { $0.path < $1.path }) {
      let effects = templates.appendingPathComponent(effectsFolder)
      guard
        let walker = FileManager.default.enumerator(at: effects, includingPropertiesForKeys: nil)
      else { continue }
      for case let file as URL in walker where file.pathExtension == "moef" {
        let relative = Array(file.pathComponents.dropFirst(effects.pathComponents.count))
        let uid = ".../" + ([effectsFolder] + relative).joined(separator: "/")
        guard relative.count >= 2, seen.insert(uid).inserted else { continue }
        let name = file.deletingPathExtension().lastPathComponent
        let template = FCPEffectTemplate(
          uid: uid, name: name,
          localizedName: try localized(name, folder: file.deletingLastPathComponent(), language),
          category: try localized(
            plain(relative[0]), folder: effects.appendingPathComponent(relative[0]), language))
        if matches(template, query) { found.append(template) }
      }
    }
    found += try fxPlugEffects(application: application, language: language).filter {
      matches($0, query)
    }
    return Array(found.sorted { $0.uid < $1.uid }.prefix(maximumResults))
  }

  static let filtersBundle =
    "Contents/PlugIns/InternalFiltersXPC.pluginkit/Contents/PlugIns/Filters.bundle/Contents"
  static let fxPlugCategory = "FxPlug"

  /// Built-in FxPlug filter list entries; only the keys used here are decoded.
  struct FxPlugList: Decodable {
    struct Entry: Decodable {
      let uuid: String?
      let displayName: String?
      let className: String?
      /// Stored as a string flag ("1"/"YES") in the bundle's list.
      let obsolete: String?
      let finalCutSimplifiedList: Bool?
    }
    let entries: [Entry]
    enum CodingKeys: String, CodingKey { case entries = "ProPlugPlugInList" }
  }

  /// Built-in FxPlug filters that Final Cut Pro offers (`finalCutSimplifiedList`),
  /// referenced in FCPXML as `FxPlug:<UUID>`. Other filters of the bundle are
  /// Motion building blocks that Final Cut Pro ignores when pasted.
  static func fxPlugEffects(application: URL, language: String) throws -> [FCPEffectTemplate] {
    let contents = application.appendingPathComponent(filtersBundle)
    let info = contents.appendingPathComponent("Info.plist")
    guard FileManager.default.fileExists(atPath: info.path) else { return [] }
    let list: FxPlugList
    do {
      list = try PropertyListDecoder().decode(FxPlugList.self, from: Data(contentsOf: info))
    } catch is DecodingError {
      throw ProAppsError.unavailable("The FxPlug filter list is malformed")
    }
    let strings = contents.appendingPathComponent("Resources/\(language).lproj/Localizable.strings")
    var names: [String: String] = [:]
    if FileManager.default.fileExists(atPath: strings.path) {
      do {
        names = try PropertyListDecoder().decode(
          [String: String].self, from: Data(contentsOf: strings))
      } catch is DecodingError {
        throw ProAppsError.unavailable("The FxPlug name table is malformed")
      }
    }
    var seen = Set<String>()
    return list.entries.filter { $0.finalCutSimplifiedList == true && $0.obsolete == nil }
      .compactMap { entry -> FCPEffectTemplate? in
        // Entries without an identity or class are list placeholders, not filters.
        // On-screen-control helpers (…OSC) are not effects.
        guard let uuid = entry.uuid, let className = entry.className,
          !className.uppercased().hasSuffix("OSC")
        else { return nil }
        let uid = FCPCarrierDocument.fxPlugPrefix + uuid
        guard seen.insert(uid).inserted else { return nil }
        return FCPEffectTemplate(
          uid: uid, name: className,
          localizedName: entry.displayName.flatMap { names[$0] } ?? className,
          category: fxPlugCategory)
      }
  }

  static func plain(_ component: String) -> String {
    component.hasSuffix(".localized") ? String(component.dropLast(".localized".count)) : component
  }

  static func matches(_ template: FCPEffectTemplate, _ query: String?) -> Bool {
    guard let query, !query.isEmpty else { return true }
    return [template.name, template.localizedName, template.category].contains {
      $0.localizedCaseInsensitiveContains(query)
    }
  }

  /// The folder's `.localized/<language>.strings` name for `key`, else `key`.
  static func localized(_ key: String, folder: URL, _ language: String) throws -> String {
    let table = folder.appendingPathComponent(".localized/\(language).strings")
    guard FileManager.default.fileExists(atPath: table.path) else { return key }
    do {
      let names = try PropertyListDecoder().decode(
        [String: String].self, from: Data(contentsOf: table))
      return names[key] ?? key
    } catch is DecodingError {
      return key
    }
  }
}

/// One published parameter of an effect template with its derived FCPXML key.
/// `verified` marks structures whose key form was confirmed against Final Cut
/// Pro's own XML (rig widgets and filters inside a layer's image node).
public struct FCPEffectTemplateParameter: Codable, Equatable, Sendable {
  public let name: String
  public let key: String
  public let structure: String
  public let verified: Bool
}

extension FCPEffectCatalog {
  static let objectTags: Set<String> = [
    "scenenode", "layer", "group", "filter", "behavior", "footage",
  ]
  static let rigSegment = "100"
  static let filterSegment = "3"
  /// Structures whose derived keys matched Final Cut Pro's own XML export.
  static let verifiedStructures: Set<String> = [
    "Rig > Widget", "layer > Image > filter", "layer > filter", "layer > Image",
    "layer > Generator > filter", "layer > Clone Layer > filter", "layer > Shape",
    "layer > group > Text Generator", "Project", "layer > group > Shape",
    "group > Generator > filter", "layer", "layer > Generator", "layer > layer > Image > filter",
    "layer > group > layer > Text", "layer > group > layer > Text Generator",
    "layer > group > filter", "layer > Shape > filter", "group > Image > filter",
    "layer > group > Text", "group > layer > Image > filter", "layer > Clone Layer",
  ]

  /// Derive FCPXML keys for the published parameters of the template `uid`.
  /// A key is "9999/" followed by the object path from the top-level node to the
  /// published object (a rig's widgets add "100", a node's filters add "3") and
  /// the published channel path.
  public static func parameters(application: URL, uid: String) throws
    -> [FCPEffectTemplateParameter]
  {
    let prefix = ".../" + effectsFolder + "/"
    guard uid.hasPrefix(prefix), !uid.contains("/../"), uid.hasSuffix(".moef") else {
      throw ProAppsError.invalid("uid must be a built-in effect template from fcp_effect_catalog")
    }
    let relative = String(uid.dropFirst(4))
    let root = application.appendingPathComponent(resources)
    let candidates = try FileManager.default.contentsOfDirectory(
      at: root, includingPropertiesForKeys: nil
    ).filter { $0.lastPathComponent.hasSuffix("Templates.localized") }
      .map { $0.appendingPathComponent(relative) }
      .filter { FileManager.default.fileExists(atPath: $0.path) }
    guard let file = candidates.sorted(by: { $0.path < $1.path }).first else {
      throw ProAppsError.unavailable("The effect template is not installed")
    }
    return try parameters(template: try Data(contentsOf: file))
  }

  static func parameters(template data: Data) throws -> [FCPEffectTemplateParameter] {
    let document: XMLDocument
    do {
      document = try XMLDocument(data: data, options: [.nodeLoadExternalEntitiesNever])
    } catch {
      throw ProAppsError.unavailable("The effect template is not readable XML")
    }
    guard let root = document.rootElement() else {
      throw ProAppsError.unavailable("The effect template has no root element")
    }
    var factories: [String: String] = [:]
    var objects: [String: XMLElement] = [:]
    var targets: [XMLElement] = []
    var stack: [XMLElement] = [root]
    while let element = stack.popLast() {
      let name = element.name ?? ""
      let id = element.attribute(forName: "id")?.stringValue
      if name == "factory", let id {
        factories[id] = element.elements(forName: "description").first?.stringValue ?? ""
      } else if objectTags.contains(name), let id {
        objects[id] = element
      } else if name == "target" {
        targets.append(element)
      }
      // Push children reversed so they are visited in document order.
      stack.append(
        contentsOf: (element.children?.compactMap { $0 as? XMLElement } ?? []).reversed())
    }
    return targets.compactMap { target -> FCPEffectTemplateParameter? in
      guard let object = target.attribute(forName: "object")?.stringValue,
        let name = target.attribute(forName: "name")?.stringValue,
        let channel = target.attribute(forName: "channel")?.stringValue,
        channel.hasPrefix("./"), let element = objects[object]
      else { return nil }
      var chain: [XMLElement] = []
      var current: XMLElement? = element
      while let node = current, objectTags.contains(node.name ?? "") {
        chain.insert(node, at: 0)
        current = node.parent as? XMLElement
      }
      func kind(_ node: XMLElement) -> String {
        let factory = node.attribute(forName: "factoryID")?.stringValue.flatMap { factories[$0] }
        if let name = node.name, ["layer", "group", "filter"].contains(name) { return name }
        return factory ?? "-"
      }
      var segments = ["9999"]
      for (offset, node) in chain.enumerated() {
        if node.name == "filter" {
          segments.append(filterSegment)
        } else if offset > 0, kind(chain[offset - 1]) == "Rig" {
          segments.append(rigSegment)
        }
        segments.append(node.attribute(forName: "id")?.stringValue ?? "")
      }
      segments.append(String(channel.dropFirst(2)))
      let structure = chain.map(kind).joined(separator: " > ")
      return FCPEffectTemplateParameter(
        name: name, key: segments.joined(separator: "/"), structure: structure,
        verified: verifiedStructures.contains(structure))
    }
  }
}

/// Numeric-aware comparison of an inspector read-back with the requested text:
/// "37" equals "37.0", and "33.333" equals a field rounded to "33.3".
public func fcpValuesMatch(_ requested: String, _ observed: String) -> Bool {
  guard let wanted = Double(requested), let seen = Double(observed) else {
    return requested == observed
  }
  let decimals = observed.split(separator: ".").dropFirst().first?.count ?? 0
  return abs(wanted - seen) <= 0.5 * pow(10, -Double(decimals)) + 1e-9
}
