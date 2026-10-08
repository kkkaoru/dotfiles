import Foundation

/// Playhead movements exposed by Final Cut Pro's Mark menu.
public enum FCPPlayheadMove: String, Codable, CaseIterable, Sendable {
  case start, end, rangeStart, rangeEnd
  case previousFrame, nextFrame, previousEdit, nextEdit, previousMarker, nextMarker

  /// Absolute moves jump to one position, so repeating them is never useful.
  public var isAbsolute: Bool {
    switch self {
    case .start, .end, .rangeStart, .rangeEnd: return true
    case .previousFrame, .nextFrame, .previousEdit, .nextEdit, .previousMarker, .nextMarker:
      return false
    }
  }
}

/// Timeline edit commands that Final Cut Pro accepts while it stays in the background.
public enum FCPEditCommand: String, Codable, CaseIterable, Sendable {
  case bladeAll, delete, deselectAll, setClipRange
  case addColorAdjustments, addColorBoard, addCrossDissolve, removeEffects

  /// Commands whose effect is observable as a changed clip count or project duration.
  public var changesTimeline: Bool {
    switch self {
    case .bladeAll, .delete: return true
    case .deselectAll, .setClipRange, .addColorAdjustments, .addColorBoard, .addCrossDissolve,
      .removeEffects:
      return false
    }
  }
}

/// Share dialog formats. Titles are resolved through the detected UI vocabulary.
public enum FCPExportFormat: String, Codable, CaseIterable, Sendable {
  case videoAndAudio, videoOnly, audioOnly
}

/// One timeline clip named by its current position and accessibility description.
/// The description guards against acting on a clip that moved since it was read.
public struct FCPClipReference: Codable, Equatable, Sendable {
  public let index: Int
  public let description: String
  public init(index: Int, description: String) {
    self.index = index
    self.description = description
  }
}

/// Export of the project open in the timeline through File > Share > Export File.
/// `allowForeground` must be true: Final Cut Pro only enables sharing while active.
public struct FCPExportRequest: Codable, Equatable, Sendable {
  public let project: String
  public let directory: String
  public let fileName: String
  public let format: FCPExportFormat?
  public let codec: String?
  public let allowForeground: Bool

  public init(
    project: String, directory: String, fileName: String, format: FCPExportFormat? = nil,
    codec: String? = nil, allowForeground: Bool
  ) {
    self.project = project
    self.directory = directory
    self.fileName = fileName
    self.format = format
    self.codec = codec
    self.allowForeground = allowForeground
  }
}

/// One live Final Cut Pro operation, executed in the UI child process.
public enum FCPRequest: Codable, Equatable, Sendable {
  case timeline(offset: Int, limit: Int)
  case select([FCPClipReference])
  case openProject(library: String?, event: String, project: String)
  case move(FCPPlayheadMove, count: Int)
  case seek(timecode: String, maximumSteps: Int)
  case edit(FCPEditCommand)
  case export(FCPExportRequest)
  case inspectorRead(tab: FCPInspectorTab?)
  case inspectorSet(FCPInspectorChange)
  case pasteEffects(FCPPasteRequest)
  case xmlExport(FCPExportRequest)
  case effectCatalog(query: String?)
  case effectParameters(uid: String)
  case closeLibrary(String)
  /// Run the wrapped request only if the timeline shows exactly this project.
  indirect case inProject(String, FCPRequest)
}

/// Inspector panes selectable by their toggle buttons.
public enum FCPInspectorTab: String, Codable, CaseIterable, Sendable {
  case video, color, audio, info, title, text
}

/// Kinds of inspector controls the tools read and write.
public enum FCPInspectorKind: String, Codable, Sendable { case value, checkbox, popup }

/// One inspector control of the selected clip, named without its control suffix.
public struct FCPInspectorParameter: Codable, Equatable, Sendable {
  public let name: String
  public let kind: FCPInspectorKind
  public let value: String?
}

/// Set a numeric/text value field (`value`) or an enable checkbox (`enabled`).
public struct FCPInspectorChange: Codable, Equatable, Sendable {
  public let parameter: String
  public let value: String?
  public let enabled: Bool?
  public let tab: FCPInspectorTab?

  public init(
    parameter: String, value: String? = nil, enabled: Bool? = nil, tab: FCPInspectorTab? = nil
  ) {
    self.parameter = parameter
    self.value = value
    self.enabled = enabled
    self.tab = tab
  }
}

/// Evidence of a carrier paste. The carrier stays in its disposable library.
public struct FCPPasteEvidence: Codable, Equatable, Sendable {
  public let carrierPath: String
  public let carrierLibrary: String
  public let carrierProject: String
  public let pastedClips: [FCPClip]
  public let mode: FCPPasteMode
  public let carrierLibraryClosed: Bool
  public let foregroundRestored: Bool
}

/// `merge` uses Edit > Paste Attributes with only the carrier's attributes
/// checked (existing effects and other attributes stay); `replace` uses Edit >
/// Paste Effects, which replaces the targets' effects and animated attributes.
public enum FCPPasteMode: String, Codable, CaseIterable, Sendable { case merge, replace }

/// Evidence of an FCPXML export written by Final Cut Pro.
public struct FCPXMLExportEvidence: Codable, Equatable, Sendable {
  public let outputPath: String
  public let written: Bool
  public let foregroundRestored: Bool
}

public struct FCPClip: Codable, Equatable, Sendable {
  public let index: Int
  public let description: String?
  public let start: String?
  public let duration: String?
  public let selected: Bool?
}

public struct FCPProjectState: Codable, Equatable, Sendable {
  public let name: String?
  public let duration: String?
  public let playhead: String?

  public init(name: String?, duration: String?, playhead: String?) {
    self.name = name
    self.duration = duration
    self.playhead = playhead
  }
}

/// Evidence of a dispatched export. Rendering continues inside Final Cut Pro after
/// the Save panel closes, so completion must be verified on the output file.
public struct FCPExportEvidence: Codable, Equatable, Sendable {
  public let outputPath: String
  public let dialogInfo: [String: String]
  public let format: String?
  public let codec: String?
  public let action: String?
  public var foregroundRestored: Bool
  public var completionVerified: Bool { false }

  private enum CodingKeys: String, CodingKey {
    case outputPath, dialogInfo, format, codec, action, foregroundRestored, completionVerified
  }

  public init(
    outputPath: String, dialogInfo: [String: String], format: String?, codec: String?,
    action: String?, foregroundRestored: Bool
  ) {
    self.outputPath = outputPath
    self.dialogInfo = dialogInfo
    self.format = format
    self.codec = codec
    self.action = action
    self.foregroundRestored = foregroundRestored
  }

  public init(from decoder: any Decoder) throws {
    let c = try decoder.container(keyedBy: CodingKeys.self)
    outputPath = try c.decode(String.self, forKey: .outputPath)
    dialogInfo = try c.decode([String: String].self, forKey: .dialogInfo)
    format = try c.decodeIfPresent(String.self, forKey: .format)
    codec = try c.decodeIfPresent(String.self, forKey: .codec)
    action = try c.decodeIfPresent(String.self, forKey: .action)
    foregroundRestored = try c.decode(Bool.self, forKey: .foregroundRestored)
  }

  public func encode(to encoder: any Encoder) throws {
    var c = encoder.container(keyedBy: CodingKeys.self)
    try c.encode(outputPath, forKey: .outputPath)
    try c.encode(dialogInfo, forKey: .dialogInfo)
    try c.encodeIfPresent(format, forKey: .format)
    try c.encodeIfPresent(codec, forKey: .codec)
    try c.encodeIfPresent(action, forKey: .action)
    try c.encode(foregroundRestored, forKey: .foregroundRestored)
    try c.encode(completionVerified, forKey: .completionVerified)
  }
}

public struct FCPResult: Codable, Equatable, Sendable {
  public var project: FCPProjectState
  public var language: String
  public var vocabularyVerified: Bool
  public var clipCount: Int?
  public var clips: [FCPClip]?
  public var steps: Int?
  public var changed: Bool?
  public var export: FCPExportEvidence?
  public var parameters: [FCPInspectorParameter]?
  public var paste: FCPPasteEvidence?
  public var xmlExport: FCPXMLExportEvidence?
  public var effects: [FCPEffectTemplate]?
  public var effectParameters: [FCPEffectTemplateParameter]?
  public var libraryClosed: Bool?

  public init(project: FCPProjectState, language: String, vocabularyVerified: Bool) {
    self.project = project
    self.language = language
    self.vocabularyVerified = vocabularyVerified
  }
}

/// Non-drop or drop-frame timecode as shown by the playhead (`HH:MM:SS:FF`).
/// Ordering compares fields, so no frame rate is needed to decide direction.
public struct FCPTimecode: Comparable, Sendable, CustomStringConvertible {
  public let hours: Int
  public let minutes: Int
  public let seconds: Int
  public let frames: Int

  static let maximumMinute = 59

  public init?(_ text: String) {
    let fields = text.split(
      omittingEmptySubsequences: false, whereSeparator: { $0 == ":" || $0 == ";" })
    guard fields.count == 4 else { return nil }
    let numbers = fields.compactMap { field -> Int? in
      guard (1...2).contains(field.count), field.allSatisfy({ $0.isASCII && $0.isNumber }) else {
        return nil
      }
      return Int(field)
    }
    guard numbers.count == 4, numbers[1] <= Self.maximumMinute,
      numbers[2] <= Self.maximumMinute
    else { return nil }
    hours = numbers[0]
    minutes = numbers[1]
    seconds = numbers[2]
    frames = numbers[3]
  }

  public var description: String {
    [hours, minutes, seconds, frames].map { String(format: "%02d", $0) }.joined(separator: ":")
  }

  public static func < (lhs: FCPTimecode, rhs: FCPTimecode) -> Bool {
    (lhs.hours, lhs.minutes, lhs.seconds, lhs.frames)
      < (rhs.hours, rhs.minutes, rhs.seconds, rhs.frames)
  }
}

/// Validated Save-panel route for an export: the destination is reached from the
/// sidebar's home item through the column browser, one display name at a time.
public struct FCPExportPlan: Equatable, Sendable {
  public let outputPath: String
  public let homeDisplayName: String
  public let components: [String]

  public static let mediaExtensions: Set<String> = ["mov", "mp4", "m4v", "m4a"]
  public static let xmlExtensions: Set<String> = ["fcpxml", "fcpxmld"]
  static let maximumFileName = 200

  /// Default location of macOS's localized names for system folders such as Movies.
  public static let systemFolderLocalizations = URL(
    fileURLWithPath: "/System/Library/CoreServices/SystemFolderLocalizations")
  static let localizedMarker = ".localized"
  static let localizationTable = "SystemFolderLocalizations.strings"

  /// Validate the request against the real filesystem. Destinations must be
  /// existing, non-hidden, symlink-free directories inside `home`, and the output
  /// file must not exist; Final Cut Pro would otherwise ask to replace it.
  /// Folders marked `.localized` are shown by the Save panel under their name in
  /// Final Cut Pro's UI `language`, so those names come from `localizations`.
  public static func make(
    _ request: FCPExportRequest, home: URL, language: String,
    localizations: URL = systemFolderLocalizations, extensions: Set<String> = mediaExtensions,
    fileManager: FileManager = .default
  ) throws -> FCPExportPlan {
    guard request.allowForeground else {
      throw ProAppsError.invalid(
        "Export activates Final Cut Pro; pass allowForeground: true to authorize it")
    }
    let name = request.fileName
    guard (1...maximumFileName).contains(name.count), !name.contains("/"), !name.contains(":"),
      !name.hasPrefix("."),
      extensions.contains(URL(fileURLWithPath: name).pathExtension.lowercased())
    else {
      let allowed = extensions.sorted().map { "." + $0 }.joined(separator: ", ")
      throw ProAppsError.invalid("fileName must be a plain visible name ending in \(allowed)")
    }
    let directory = URL(fileURLWithPath: request.directory)
    guard request.directory.hasPrefix("/"),
      directory.standardizedFileURL.path == request.directory,
      directory.resolvingSymlinksInPath().path == request.directory
    else {
      throw ProAppsError.invalid("directory must be an absolute, normalized, symlink-free path")
    }
    let homePath = home.standardizedFileURL.resolvingSymlinksInPath()
    let base = homePath.pathComponents
    let parts = directory.pathComponents
    guard parts.count >= base.count, Array(parts.prefix(base.count)) == base else {
      throw ProAppsError.invalid("directory must be inside the home folder")
    }
    let relative = Array(parts.dropFirst(base.count))
    guard relative.allSatisfy({ !$0.hasPrefix(".") }) else {
      throw ProAppsError.invalid("Hidden folders are not reachable in the Save panel")
    }
    var isDirectory: ObjCBool = false
    guard fileManager.fileExists(atPath: directory.path, isDirectory: &isDirectory),
      isDirectory.boolValue
    else { throw ProAppsError.invalid("directory does not exist") }
    let output = directory.appendingPathComponent(name)
    guard !fileManager.fileExists(atPath: output.path) else {
      throw ProAppsError.invalid("The output file already exists; choose a new fileName")
    }
    let table = try localizedNames(language: language, in: localizations)
    var current = homePath
    let names = relative.map { part -> String in
      current = current.appendingPathComponent(part)
      let marked = fileManager.fileExists(
        atPath: current.appendingPathComponent(localizedMarker).path)
      return marked ? table[part] ?? part : part
    }
    return FCPExportPlan(
      outputPath: output.path, homeDisplayName: homePath.lastPathComponent, components: names)
  }

  /// Read the system folder name table for `language`; a missing table means
  /// the language shows unlocalized names.
  static func localizedNames(language: String, in directory: URL) throws -> [String: String] {
    let url = directory.appendingPathComponent("\(language).lproj")
      .appendingPathComponent(localizationTable)
    guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
    do {
      return try PropertyListDecoder().decode([String: String].self, from: Data(contentsOf: url))
    } catch is DecodingError {
      throw ProAppsError.unavailable("The system folder localization table is malformed")
    }
  }
}

/// Checkbox titles of the Paste Attributes dialog.
public struct FCPAttributeLabels: Equatable, Sendable {
  let position: String
  let rotation: String
  let scale: String
  let anchor: String
  let opacity: String
  let headers: Set<String>
  let others: Set<String>
  let maintainTiming: String
  let paste: String
}

/// Localized titles for the menus and dialog controls that expose no stable
/// identifier. Japanese and English titles were both exercised on Final Cut Pro
/// 12.3 (English via a relaunch with -AppleLanguages (en)).
public struct FCPVocabulary: Equatable, Sendable {
  public let language: String
  public let verified: Bool
  let menuBarKey: String
  /// File > Share; the default destination is found by its marker because
  /// destination names are user-editable and not localized with the UI.
  let share: [String]
  let defaultDestinationMarker: String
  let openClip: [String]
  let moves: [FCPPlayheadMove: [String]]
  let edits: [FCPEditCommand: [String]]
  let settingsTab: String
  let next: String
  let cancel: String
  let save: String
  let formatLabel: String
  let codecLabel: String
  let actionLabel: String
  let formats: [FCPExportFormat: String]
  let saveOnly: String
  let copy: [String]
  let pasteEffects: [String]
  let xmlExport: [String]
  let tabs: [FCPInspectorTab: String]
  let valueSuffix: String
  let checkboxSuffix: String
  let popupSuffix: String
  let pasteAttributes: [String]
  let closeLibraryPrefix: String
  let closeLibrarySuffix: String
  let attributeLabels: FCPAttributeLabels

  public static let japanese = FCPVocabulary(
    language: "ja", verified: true, menuBarKey: "ファイル",
    share: ["ファイル", "共有"], defaultDestinationMarker: "（デフォルト）",
    openClip: ["クリップ", "クリップを開く"],
    moves: [
      .start: ["マーク", "移動", "開始"], .end: ["マーク", "移動", "終了"],
      .rangeStart: ["マーク", "移動", "範囲開始点"], .rangeEnd: ["マーク", "移動", "範囲終了点"],
      .previousFrame: ["マーク", "前へ", "フレーム"], .nextFrame: ["マーク", "次へ", "フレーム"],
      .previousEdit: ["マーク", "前へ", "編集"], .nextEdit: ["マーク", "次へ", "編集"],
      .previousMarker: ["マーク", "前へ", "マーカー"], .nextMarker: ["マーク", "次へ", "マーカー"],
    ],
    edits: [
      .bladeAll: ["トリム", "すべてをブレード"], .delete: ["編集", "削除"],
      .deselectAll: ["編集", "すべてを選択解除"], .setClipRange: ["マーク", "クリップ範囲を設定"],
      .addColorAdjustments: ["編集", "カラー調整を追加"], .addColorBoard: ["編集", "カラーボードを追加"],
      .addCrossDissolve: ["編集", "クロスディゾルブを追加"], .removeEffects: ["編集", "エフェクトを削除"],
    ],
    settingsTab: "設定", next: "次へ…", cancel: "キャンセル", save: "保存",
    formatLabel: "フォーマット:", codecLabel: "ビデオコーデック:", actionLabel: "操作:",
    formats: [.videoAndAudio: "ビデオとオーディオ", .videoOnly: "ビデオのみ", .audioOnly: "オーディオのみ"],
    saveOnly: "保存のみ", copy: ["編集", "コピー"], pasteEffects: ["編集", "エフェクトをペースト"],
    xmlExport: ["ファイル", "XMLを書き出す…"],
    tabs: [
      .video: "ビデオ", .color: "カラー", .audio: "オーディオ", .info: "情報", .title: "タイトル",
      .text: "テキスト",
    ],
    valueSuffix: "スクラバー", checkboxSuffix: "チェックボックス", popupSuffix: "ポップアップ",
    pasteAttributes: ["編集", "パラメータをペースト…"], closeLibraryPrefix: "ライブラリ“",
    closeLibrarySuffix: "”を閉じる",
    attributeLabels: FCPAttributeLabels(
      position: "位置", rotation: "回転", scale: "調整", anchor: "アンカー", opacity: "合成",
      headers: ["エフェクト", "トランスフォーム"], others: ["クロップ", "歪み", "空間適合"],
      maintainTiming: "保持", paste: "ペースト"))

  public static let english = FCPVocabulary(
    language: "en", verified: true, menuBarKey: "File",
    share: ["File", "Share"], defaultDestinationMarker: "(default)",
    openClip: ["Clip", "Open Clip"],
    moves: [
      .start: ["Mark", "Go to", "Beginning"], .end: ["Mark", "Go to", "End"],
      .rangeStart: ["Mark", "Go to", "Range Start"], .rangeEnd: ["Mark", "Go to", "Range End"],
      .previousFrame: ["Mark", "Previous", "Frame"], .nextFrame: ["Mark", "Next", "Frame"],
      .previousEdit: ["Mark", "Previous", "Edit"], .nextEdit: ["Mark", "Next", "Edit"],
      .previousMarker: ["Mark", "Previous", "Marker"], .nextMarker: ["Mark", "Next", "Marker"],
    ],
    edits: [
      .bladeAll: ["Trim", "Blade All"], .delete: ["Edit", "Delete"],
      .deselectAll: ["Edit", "Deselect All"], .setClipRange: ["Mark", "Set Clip Range"],
      .addColorAdjustments: ["Edit", "Add Color Adjustment"],
      .addColorBoard: ["Edit", "Add Color Board"],
      .addCrossDissolve: ["Edit", "Add Cross Dissolve"],
      .removeEffects: ["Edit", "Remove Effects"],
    ],
    settingsTab: "Settings", next: "Next…", cancel: "Cancel", save: "Save",
    formatLabel: "Format:", codecLabel: "Video Codec:", actionLabel: "Action:",
    formats: [
      .videoAndAudio: "Video and Audio", .videoOnly: "Video Only", .audioOnly: "Audio Only",
    ],
    saveOnly: "Save only", copy: ["Edit", "Copy"], pasteEffects: ["Edit", "Paste Effects"],
    xmlExport: ["File", "Export XML…"],
    tabs: [
      .video: "Video", .color: "Color", .audio: "Audio", .info: "Info", .title: "Title",
      .text: "Text",
    ],
    valueSuffix: " scrubber", checkboxSuffix: " check box", popupSuffix: " pop up",
    pasteAttributes: ["Edit", "Paste Attributes…"], closeLibraryPrefix: "Close Library “",
    closeLibrarySuffix: "”",
    attributeLabels: FCPAttributeLabels(
      position: "Position", rotation: "Rotation", scale: "Scale", anchor: "Anchor",
      opacity: "Compositing", headers: ["Effects", "Transform"],
      others: ["Crop", "Distort", "Spatial Conform"], maintainTiming: "Maintain",
      paste: "Paste"))

  /// Pick the vocabulary whose File menu title appears in the menu bar.
  public static func detect(menuBarTitles: [String]) -> FCPVocabulary? {
    [japanese, english].first { menuBarTitles.contains($0.menuBarKey) }
  }

  func path(_ move: FCPPlayheadMove) throws -> [String] {
    guard let path = moves[move] else { throw ProAppsError.invalid("Unsupported move") }
    return path
  }

  func path(_ command: FCPEditCommand) throws -> [String] {
    guard let path = edits[command] else { throw ProAppsError.invalid("Unsupported command") }
    return path
  }

  func title(_ tab: FCPInspectorTab) throws -> String {
    guard let title = tabs[tab] else { throw ProAppsError.invalid("Unsupported tab") }
    return title
  }

  func title(_ format: FCPExportFormat) throws -> String {
    guard let title = formats[format] else { throw ProAppsError.invalid("Unsupported format") }
    return title
  }
}
