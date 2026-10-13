import Foundation

extension UIAutomation {
  /// Verify the current directory before any filename write or Save dispatch.
  /// This is observation plus menu opening/closing, never directory navigation.
  func currentSaveDirectoryMatches(_ plan: FCPExportPlan, sheet: UIHandle, language: String)
    async throws -> Bool
  {
    let directory = URL(fileURLWithPath: plan.outputPath).deletingLastPathComponent()
    try FCPExportPlan.requireNewDestination(URL(fileURLWithPath: plan.outputPath))
    let popup = try uniqueNode(in: sheet, depth: Self.textSearchDepth, name: "Save location") {
      try $0.role == "AXPopUpButton" && string(.identifier, of: $0.handle) == "where popup"
    }
    let ancestry = try Self.saveDirectoryAncestry(
      directory, language: language, localizations: folderLocalizations)
    let leaf = ancestry.first
    guard try text(.value, of: popup) == leaf else { return false }
    try perform(.press, on: popup)
    let menu: UIHandle
    do {
      guard
        let observedMenu = try await finalCutWait(
          Self.finalCutShortWait,
          {
            try children(of: popup).first { try string(.role, of: $0) == "AXMenu" }
          })
      else { throw ProAppsError.unavailable("Save phase ancestry-menu: menu is unavailable") }
      menu = observedMenu
      try verifySaveAncestry(menu, expected: ancestry)
    } catch {
      do {
        let owned = try children(of: popup).filter { try string(.role, of: $0) == "AXMenu" }
        if owned.count == 1, let opened = owned.first { try perform(.cancel, on: opened) }
      } catch let cleanup {
        throw ProAppsError.unavailable(
          "Save directory verification failed: \(error); menu cleanup failed: \(cleanup)")
      }
      throw error
    }
    try await closeVerifiedSaveMenu(menu, popup: popup)
    return true
  }

  func verifySaveAncestry(_ menu: UIHandle, expected: [String]) throws {
    let entries = try children(of: menu)
    guard entries.count <= Self.maximumSearchNodes else {
      throw ProAppsError.unavailable("Save phase ancestry-menu: too many entries")
    }
    let titles = try entries.prefix(expected.count).map { try string(.title, of: $0) ?? "" }
    guard titles == expected else {
      throw ProAppsError.unavailable(
        "Save phase ancestry-menu: full directory mismatch; expected=\(expected); observed=\(titles)"
      )
    }
  }

  func closeVerifiedSaveMenu(_ menu: UIHandle, popup: UIHandle) async throws {
    try perform(.cancel, on: menu)
    guard
      try await finalCutWait(
        Self.finalCutShortWait,
        {
          try children(of: popup).isEmpty ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("Save location menu stayed open") }
  }

  /// Finder/Save panels omit the synthetic /Volumes ancestor above a mounted root.
  static func saveDirectoryAncestry(
    _ directory: URL, language: String,
    localizations: URL = FCPExportPlan.systemFolderLocalizations
  ) throws -> [String] {
    guard let volume = try directory.resourceValues(forKeys: [.volumeURLKey]).volume else {
      throw ProAppsError.unavailable("Save directory volume is unavailable")
    }
    // APFS firmlinks can expose a data-volume path through the root namespace.
    let root =
      directory.pathComponents.starts(with: volume.pathComponents)
      ? volume : URL(fileURLWithPath: "/")
    guard
      let rootName = try root.resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName
    else {
      throw ProAppsError.unavailable("Save root volume display name is unavailable")
    }
    let table = try FCPExportPlan.localizedNames(language: language, in: localizations)
    var current = directory
    var result: [String] = []
    while true {
      if current.standardizedFileURL.path == root.standardizedFileURL.path {
        result.append(rootName)
        return result
      }
      let part = current.lastPathComponent
      let marked = FileManager.default.fileExists(
        atPath: current.appendingPathComponent(".localized").path)
      result.append(marked ? table[part] ?? part : part)
      current = current.deletingLastPathComponent()
    }
  }
}
