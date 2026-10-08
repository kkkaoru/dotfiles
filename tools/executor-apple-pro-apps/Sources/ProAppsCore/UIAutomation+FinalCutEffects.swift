import Foundation

/// Effects, parameters and keyframes for live Final Cut Pro control.
///
/// - Inspector value fields, enable checkboxes and pop-ups are read and written
///   with Final Cut Pro in the background.
/// - Effects and keyframes are applied by importing a generated carrier clip into
///   a disposable library, copying it and running Edit > Paste Effects on the
///   target clips (the user approved the clipboard use on 2026-10-08).
/// - FCPXML export activates Final Cut Pro like Share does.
extension UIAutomation {
  static var finalCutImportWait: Double { 60 }
  static var finalCutInspectorDescription: String { "inspector" }
  static var finalCutCarrierPrefix: String { "carrier-" }

  // MARK: Inspector

  /// The clip inspector's scroll area. Final Cut Pro refreshes the inspector
  /// asynchronously after selection or focus changes, so wait briefly for it.
  func inspector(in window: UIHandle) async throws -> UIHandle {
    let found = try await finalCutWait(Self.finalCutShortWait) { () throws -> UIHandle? in
      let areas = try finalCutNodes(
        from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles
      ).filter {
        try $0.role == "AXScrollArea"
          && string(.description, of: $0.handle) == Self.finalCutInspectorDescription
      }
      return areas.count == 1 ? areas[0].handle : nil
    }
    guard let found else {
      throw ProAppsError.unavailable("The inspector is not shown; select a clip first")
    }
    return found
  }

  /// Press the inspector pane toggle for `tab` unless it is already selected.
  func showInspector(_ tab: FCPInspectorTab, window: UIHandle, vocabulary: FCPVocabulary)
    async throws
  {
    let title = try vocabulary.title(tab)
    let toggle = try uniqueNode(
      in: window, depth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles,
      name: "Inspector tab \(title)"
    ) { try $0.role == "AXCheckBox" && string(.title, of: $0.handle) == title }
    guard try bool(.value, of: toggle) != true else { return }
    try perform(.press, on: toggle)
    guard
      try await finalCutWait(
        Self.finalCutShortWait,
        {
          try bool(.value, of: toggle) == true ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("The inspector did not switch to \(title)") }
  }

  func inspectorParameters(tab: FCPInspectorTab?, window: UIHandle, vocabulary: FCPVocabulary)
    async throws -> [FCPInspectorParameter]
  {
    try await withInspectorTab(tab, window: window, vocabulary: vocabulary) {
      try controls(in: try await inspector(in: window), vocabulary: vocabulary).map(\.parameter)
    }
  }

  /// The inspector pane currently selected, if one of the known toggles is on.
  func currentInspectorTab(window: UIHandle, vocabulary: FCPVocabulary) throws -> FCPInspectorTab? {
    let toggles = try finalCutNodes(
      from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles
    ).filter { $0.role == "AXCheckBox" }
    for tab in FCPInspectorTab.allCases {
      let title = try vocabulary.title(tab)
      if try toggles.contains(where: {
        try string(.title, of: $0.handle) == title && bool(.value, of: $0.handle) == true
      }) {
        return tab
      }
    }
    return nil
  }

  /// Run `body` with `tab` shown, then show the previously selected pane again
  /// (also after a failure) so the user's inspector is left as it was.
  func withInspectorTab<T>(
    _ tab: FCPInspectorTab?, window: UIHandle, vocabulary: FCPVocabulary,
    _ body: () async throws -> T
  ) async throws -> T {
    guard let tab else { return try await body() }
    let previous = try currentInspectorTab(window: window, vocabulary: vocabulary)
    try await showInspector(tab, window: window, vocabulary: vocabulary)
    let value: T
    do {
      value = try await body()
    } catch {
      if let previous, previous != tab {
        do {
          try await showInspector(previous, window: window, vocabulary: vocabulary)
        } catch let restoreError {
          throw ProAppsError.unavailable(
            "\(error); restoring the inspector pane failed: \(restoreError)")
        }
      }
      throw error
    }
    if let previous, previous != tab {
      try await showInspector(previous, window: window, vocabulary: vocabulary)
    }
    return value
  }

  /// Inspector controls named by their accessibility description minus the
  /// localized suffix ("不透明度スクラバー" → "不透明度").
  func controls(in inspector: UIHandle, vocabulary: FCPVocabulary) throws
    -> [(parameter: FCPInspectorParameter, handle: UIHandle)]
  {
    let kinds: [(String, String, FCPInspectorKind)] = [
      ("AXTextField", vocabulary.valueSuffix, .value),
      ("AXCheckBox", vocabulary.checkboxSuffix, .checkbox),
      ("AXPopUpButton", vocabulary.popupSuffix, .popup),
    ]
    return try children(of: inspector).compactMap { child in
      let role = try string(.role, of: child)
      guard let description = try string(.description, of: child),
        let (_, suffix, kind) = kinds.first(where: { $0.0 == role }),
        description.hasSuffix(suffix), description.count > suffix.count
      else { return nil }
      let name = String(description.dropLast(suffix.count))
      return (
        FCPInspectorParameter(name: name, kind: kind, value: try text(.value, of: child)), child
      )
    }
  }

  /// Write one inspector value field (confirmed and read back numerically) or
  /// toggle one enable checkbox to the requested state.
  func setInspector(_ change: FCPInspectorChange, window: UIHandle, vocabulary: FCPVocabulary)
    async throws -> FCPInspectorParameter
  {
    guard (change.value == nil) != (change.enabled == nil) else {
      throw ProAppsError.invalid("Give exactly one of value or enabled")
    }
    return try await withInspectorTab(change.tab, window: window, vocabulary: vocabulary) {
      try await applyInspector(change, window: window, vocabulary: vocabulary)
    }
  }

  func applyInspector(_ change: FCPInspectorChange, window: UIHandle, vocabulary: FCPVocabulary)
    async throws -> FCPInspectorParameter
  {
    let inspector = try await inspector(in: window)
    let kind: FCPInspectorKind = change.value == nil ? .checkbox : .value
    let matches = try controls(in: inspector, vocabulary: vocabulary).filter {
      $0.parameter.kind == kind
        && $0.parameter.name.caseInsensitiveCompare(change.parameter) == .orderedSame
    }
    guard matches.count == 1 else {
      throw ProAppsError.invalid(
        "Parameter \(change.parameter) matched \(matches.count) inspector controls")
    }
    let control = matches[0].handle
    if let value = change.value {
      try mappedBackend { try backend.set(.value, to: .string(value), on: control) }
      try perform(.confirm, on: control)
      guard let observed = try text(.value, of: control), fcpValuesMatch(value, observed) else {
        throw ProAppsError.unavailable(
          "Final Cut Pro did not accept \(value) for \(change.parameter)")
      }
    } else if let enabled = change.enabled, try bool(.value, of: control) != enabled {
      try perform(.press, on: control)
      guard
        try await finalCutWait(
          Self.finalCutShortWait,
          {
            try bool(.value, of: control) == enabled ? true : nil
          }) != nil
      else { throw ProAppsError.unavailable("\(change.parameter) did not change to \(enabled)") }
    }
    // Handles are re-issued on every read, so read the control back by name.
    let name = matches[0].parameter.name
    guard
      let updated = try controls(in: inspector, vocabulary: vocabulary).first(where: {
        $0.parameter.kind == kind && $0.parameter.name == name
      })
    else { throw ProAppsError.unavailable("The inspector control disappeared") }
    return updated.parameter
  }

  // MARK: Carrier paste

  /// Import a generated carrier, copy it and paste it onto the targets: merge
  /// (Paste Attributes with only the carrier's attributes) or replace (Paste
  /// Effects). The import runs in the background; opening projects, copying and
  /// pasting run in the approved foreground scope, and the carrier library is
  /// closed afterwards unless the caller keeps it open.
  func pasteEffects(
    _ request: FCPPasteRequest, window: UIHandle, vocabulary: FCPVocabulary,
    process: UIRunningProcess
  ) async throws -> FCPPasteEvidence {
    var isDirectory: ObjCBool = false
    let work = URL(fileURLWithPath: request.workDirectory)
    guard request.workDirectory.hasPrefix("/"),
      FileManager.default.fileExists(atPath: work.path, isDirectory: &isDirectory),
      isDirectory.boolValue
    else { throw ProAppsError.invalid("workDirectory must be an existing absolute folder") }
    guard !request.targets.isEmpty else { throw ProAppsError.invalid("Name at least one target") }
    let name = Self.finalCutCarrierPrefix + UUID().uuidString.prefix(8).lowercased()
    let document = try FCPCarrierDocument.make(request.carrier, workDirectory: work, project: name)
    let file = try Files.writeNew(
      Data(document.xml.utf8), to: work.appendingPathComponent(name + ".fcpxml").path,
      extensions: ["fcpxml"])
    guard let application = backend.applicationURL(bundleID: process.bundleID) else {
      throw ProAppsError.unavailable("Selected app edition is not installed")
    }
    try await mappedAsyncBackend {
      try await backend.open(document: file, applicationAt: application)
    }
    guard
      try await finalCutWait(
        Self.finalCutImportWait, interval: 1,
        {
          try eventRowIfPresent(library: document.library, event: document.event, in: window)
        }) != nil
    else { throw ProAppsError.unavailable("Final Cut Pro did not import the carrier library") }
    let (pasted, restored) = try await inForeground(vocabulary: vocabulary, process: process) {
      () async throws -> [FCPClip] in
      try await openProject(
        library: document.library, event: document.event, project: name, window: window,
        vocabulary: vocabulary, process: process)
      let carrierTimeline = try requireTimeline(try finalCutLandmarks(in: window))
      guard let carrierClip = try timelineClips(carrierTimeline).first,
        let description = try string(.description, of: carrierClip)
      else { throw ProAppsError.unavailable("The carrier project has no clip") }
      _ = try selectClips(
        [FCPClipReference(index: 0, description: description)], timeline: carrierTimeline)
      try pressFinalCutMenu(vocabulary.copy, process: process)
      try await openProject(
        library: request.library, event: request.event, project: request.project, window: window,
        vocabulary: vocabulary, process: process)
      let timeline = try requireTimeline(try finalCutLandmarks(in: window))
      let pasted = try selectClips(request.targets, timeline: timeline)
      try mappedBackend { try backend.set(.focused, to: .bool(true), on: timeline) }
      switch request.mode {
      case .merge:
        try await pasteAttributes(
          request.carrier, vocabulary: vocabulary, process: process)
      case .replace:
        try pressFinalCutMenu(vocabulary.pasteEffects, process: process)
      }
      if request.closeCarrierLibrary {
        try await closeLibrary(
          document.library, window: window, vocabulary: vocabulary, process: process)
        // Closing moves the browser selection to another library; reselect the
        // targets so the inspector shows the pasted clips again.
        let refreshed = try requireTimeline(try finalCutLandmarks(in: window))
        try mappedBackend { try backend.set(.focused, to: .bool(true), on: refreshed) }
        try mappedBackend { try backend.set(.selectedChildren, to: .elements([]), on: refreshed) }
        _ = try selectClips(request.targets, timeline: refreshed)
      }
      return pasted
    }
    return FCPPasteEvidence(
      carrierPath: file.path, carrierLibrary: document.library, carrierProject: name,
      pastedClips: pasted, mode: request.mode, carrierLibraryClosed: request.closeCarrierLibrary,
      foregroundRestored: restored)
  }

  /// Edit > Paste Attributes with exactly the carrier's attributes checked and
  /// keyframe timing maintained, so existing effects and attributes stay.
  func pasteAttributes(
    _ carrier: FCPCarrierSpec, vocabulary: FCPVocabulary, process: UIRunningProcess
  ) async throws {
    let item = try finalCutMenuItem(vocabulary.pasteAttributes, process: process)
    guard
      try await finalCutWait(
        Self.finalCutShortWait, { try bool(.enabled, of: item) == true ? true : nil }) != nil
    else { throw ProAppsError.unavailable("Paste Attributes is disabled; the clipboard changed") }
    try perform(.press, on: item)
    guard
      let dialog = try await finalCutWait(
        Self.finalCutDialogWait, { try exportDialog(process: process) })
    else { throw ProAppsError.unavailable("The Paste Attributes dialog did not open") }
    let labels = vocabulary.attributeLabels
    let wanted: [String: Bool] = [
      labels.position: carrier.position != nil, labels.rotation: carrier.rotation != nil,
      labels.scale: carrier.scale != nil, labels.anchor: carrier.anchor != nil,
      labels.opacity: carrier.opacity != nil,
    ]
    let boxes = try finalCutNodes(from: dialog, maxDepth: Self.textSearchDepth, skipping: [])
      .filter { $0.role == "AXCheckBox" && $0.parent != dialog }
    for box in boxes {
      guard let title = try string(.title, of: box.handle), !labels.headers.contains(title)
      else { continue }
      let want = wanted[title] ?? (labels.others.contains(title) ? false : !carrier.effects.isEmpty)
      guard try bool(.value, of: box.handle) != want else { continue }
      try perform(.press, on: box.handle)
      guard
        try await finalCutWait(
          Self.finalCutShortWait, { try bool(.value, of: box.handle) == want ? true : nil }) != nil
      else { throw ProAppsError.unavailable("Paste Attributes did not toggle \(title)") }
    }
    let maintain = try uniqueNode(
      in: dialog, depth: Self.textSearchDepth, name: "Maintain timing"
    ) { try $0.role == "AXRadioButton" && string(.title, of: $0.handle) == labels.maintainTiming }
    if try bool(.value, of: maintain) != true { try perform(.press, on: maintain) }
    try perform(.press, on: try button(labels.paste, in: dialog))
    guard
      try await finalCutWait(
        Self.finalCutDialogWait, { try exportDialog(process: process) == nil ? true : nil }) != nil
    else { throw ProAppsError.unavailable("The Paste Attributes dialog stayed open") }
  }

  /// Close one open library by its sidebar name (File > Close Library “name”).
  /// The menu title must name exactly that library, so another library is
  /// never closed by mistake.
  func closeLibrary(
    _ library: String, window: UIHandle, vocabulary: FCPVocabulary, process: UIRunningProcess
  ) async throws {
    let rows = try finalCutNodes(
      from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles
    ).filter { $0.role == "AXOutline" }.flatMap { outline in
      try children(of: outline.handle).compactMap { row -> (UIHandle, UIHandle)? in
        guard let value = try read(.disclosureLevel, of: row), case .number(0) = value,
          try finalCutTexts(row, depth: Self.textSearchDepth).contains(library)
        else { return nil }
        return (outline.handle, row)
      }
    }
    guard rows.count == 1 else {
      throw ProAppsError.invalid("Library \(library) matched \(rows.count) sidebar rows")
    }
    try mappedBackend { try backend.set(.selectedRows, to: .elements([rows[0].1]), on: rows[0].0) }
    try mappedBackend { try backend.set(.focused, to: .bool(true), on: rows[0].0) }
    let title = vocabulary.closeLibraryPrefix + library + vocabulary.closeLibrarySuffix
    let file = try menuContainer(path: [vocabulary.share[0]], process: process)
    guard
      let item = try await finalCutWait(
        Self.finalCutShortWait,
        {
          try menuChildren(of: file).first {
            try string(.title, of: $0) == title && bool(.enabled, of: $0) == true
          }
        })
    else { throw ProAppsError.unavailable("Final Cut Pro does not offer to close \(library)") }
    try perform(.press, on: item)
    guard
      try await finalCutWait(
        Self.finalCutDialogWait,
        {
          try finalCutNodes(
            from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles
          ).filter { $0.role == "AXOutline" }.allSatisfy { outline in
            try children(of: outline.handle).allSatisfy { row in
              try !finalCutTexts(row, depth: Self.textSearchDepth).contains(library)
            }
          } ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("Final Cut Pro kept the library \(library) open") }
  }

  /// The event row if the library and event are listed; nil while importing.
  func eventRowIfPresent(library: String, event: String, in window: UIHandle) throws
    -> (UIHandle, UIHandle)?
  {
    let rows = try eventRows(library: library, event: event, in: window)
    return rows.count == 1 ? rows[0] : nil
  }

  // MARK: FCPXML export

  func exportXML(
    _ request: FCPExportRequest, plan: FCPExportPlan, timeline: UIHandle,
    vocabulary: FCPVocabulary, process: UIRunningProcess
  ) async throws -> FCPXMLExportEvidence {
    let (_, restored) = try await inForeground(vocabulary: vocabulary, process: process) {
      try mappedBackend { try backend.set(.focused, to: .bool(true), on: timeline) }
      let item = try finalCutMenuItem(vocabulary.xmlExport, process: process)
      guard
        try await finalCutWait(
          Self.finalCutShortWait,
          {
            try bool(.enabled, of: item) == true ? true : nil
          }) != nil
      else { throw ProAppsError.unavailable("Final Cut Pro kept Export XML disabled while active") }
      try perform(.press, on: item)
      guard
        let dialog = try await finalCutWait(
          Self.finalCutDialogWait,
          {
            try exportDialog(process: process)
          })
      else { throw ProAppsError.unavailable("The Export XML panel did not open") }
      guard
        try finalCutNodes(from: dialog, maxDepth: Self.textSearchDepth, skipping: ["AXBrowser"])
          .contains(where: {
            try $0.role == "AXStaticText" && text(.value, of: $0.handle) == request.project
          })
      else { throw ProAppsError.invalid("The Export XML panel names a different project") }
      try await saveAs(plan, sheet: dialog, vocabulary: vocabulary)
      guard
        try await finalCutWait(
          Self.finalCutDialogWait,
          {
            try exportDialog(process: process) == nil ? true : nil
          }) != nil
      else { throw ProAppsError.unavailable("The Export XML panel stayed open") }
    }
    // Final Cut Pro writes the document after the panel closes; a bundle is
    // complete once its Info.fcpxml exists.
    let output = URL(fileURLWithPath: plan.outputPath)
    let document =
      output.pathExtension == "fcpxmld" ? output.appendingPathComponent("Info.fcpxml") : output
    let written = try await finalCutWait(Self.finalCutDialogWait) {
      FileManager.default.fileExists(atPath: document.path) ? true : nil
    }
    return FCPXMLExportEvidence(
      outputPath: plan.outputPath, written: written != nil, foregroundRestored: restored)
  }
}
