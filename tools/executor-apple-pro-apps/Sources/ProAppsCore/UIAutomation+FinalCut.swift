import Foundation

/// Final Cut Pro landmarks found by one bounded traversal of the main window.
struct FCPLandmarks {
  var timeline: UIHandle?
  var projectName: UIHandle?
  var projectInfo: UIHandle?
}

/// One traversed element with its parent, used where a parent must be written.
struct FCPNode {
  let handle: UIHandle
  let parent: UIHandle?
  let role: String?
}

/// Live Final Cut Pro timeline control built on the same Accessibility backend.
/// Everything except export runs with Final Cut Pro in the background; export
/// activates it only when the request carries `allowForeground: true`.
extension UIAutomation {
  static var finalCutMaximumMoves: Int { 600 }
  static var finalCutMaximumSeekSteps: Int { 216_000 }
  static var finalCutMaximumPage: Int { 1_000 }
  static var finalCutMaximumDialogInfo: Int { 32 }
  static var finalCutStepPoll: Double { 0.02 }
  static var finalCutStepSettle: Double { 1 }
  static var finalCutShortWait: Double { 5 }
  static var finalCutDialogWait: Double { 15 }
  static var finalCutTimelineIdentifierPrefix: String { "editor/timelineContainer/toolbar/" }
  static var finalCutSkippedRoles: Set<String> {
    ["AXTable", "AXOutline", "AXList", "AXLayoutArea", "AXBrowser", "AXMenu"]
  }

  static var finalCutBusyWait: Double { 10 }

  func finalCut(_ request: FCPRequest, process: UIRunningProcess) async throws -> FCPResult {
    let vocabulary = try await responsiveVocabulary(process)
    let window = try finalCutMainWindow(process)
    let landmarks = try finalCutLandmarks(in: window)
    var result = FCPResult(
      project: try projectState(landmarks), language: vocabulary.language,
      vocabularyVerified: vocabulary.verified)
    switch request {
    case .inProject(let expected, let inner):
      if case .inProject = inner { throw ProAppsError.invalid("Nested project guards") }
      guard result.project.name == expected else {
        throw ProAppsError.invalid(
          "The timeline shows \(result.project.name ?? "no project"), not \(expected); nothing was changed"
        )
      }
      return try await finalCut(inner, process: process)
    case .timeline(let offset, let limit):
      guard offset >= 0, (1...Self.finalCutMaximumPage).contains(limit) else {
        throw ProAppsError.invalid("offset must be ≥ 0 and limit 1–1000")
      }
      let clips = try timelineClips(try requireTimeline(landmarks))
      result.clipCount = clips.count
      result.clips = try clips.enumerated().dropFirst(offset).prefix(limit).map {
        try clipSummary($0.element, index: $0.offset)
      }
    case .select(let references):
      result.clips = try selectClips(references, timeline: try requireTimeline(landmarks))
      result.changed = true
    case .openProject(let library, let event, let project):
      // Open Clip follows Final Cut Pro's real keyboard focus, which only an
      // active app moves reliably; run it in the approved foreground scope.
      _ = try await inForeground(vocabulary: vocabulary, process: process) {
        try await openProject(
          library: library, event: event, project: project, window: window,
          vocabulary: vocabulary, process: process)
      }
      result.project = try projectState(try finalCutLandmarks(in: window))
      result.changed = true
    case .move(let move, let count):
      guard (1...Self.finalCutMaximumMoves).contains(count), !move.isAbsolute || count == 1 else {
        throw ProAppsError.invalid("count must be 1–600, and 1 for absolute moves")
      }
      let indicator = try playheadIndicator(try requireTimeline(landmarks))
      let item = try finalCutMenuItem(try vocabulary.path(move), process: process)
      var moved = 0
      for _ in 0..<count {
        guard try await step(item, indicator: indicator) != nil else { break }
        moved += 1
      }
      result.steps = moved
      result.project = try projectState(landmarks)
    case .seek(let timecode, let maximumSteps):
      guard let target = FCPTimecode(timecode) else {
        throw ProAppsError.invalid("timecode must be HH:MM:SS:FF")
      }
      guard (1...Self.finalCutMaximumSeekSteps).contains(maximumSteps) else {
        throw ProAppsError.invalid("maximumSteps must be 1–216000")
      }
      result.steps = try await seek(
        to: target, maximumSteps: maximumSteps, timeline: try requireTimeline(landmarks),
        vocabulary: vocabulary, process: process)
      result.project = try projectState(landmarks)
    case .edit(let command):
      let timeline = try requireTimeline(landmarks)
      let before = (try timelineClips(timeline).count, result.project.duration)
      try pressFinalCutMenu(try vocabulary.path(command), process: process)
      var after = before
      if command.changesTimeline {
        _ = try await finalCutWait(Self.finalCutShortWait) { () throws -> Bool? in
          after = (try timelineClips(timeline).count, try projectState(landmarks).duration)
          return after != before ? true : nil
        }
      }
      result.changed = command.changesTimeline ? after != before : nil
      result.clipCount = after.0
      result.project = try projectState(landmarks)
    case .export(let export):
      let plan = try FCPExportPlan.make(
        export, home: home, language: vocabulary.language, localizations: folderLocalizations)
      guard result.project.name == export.project else {
        throw ProAppsError.invalid(
          "The timeline shows a different project; open the requested project first")
      }
      result.export = try await exportProject(
        export, plan: plan, timeline: try requireTimeline(landmarks), vocabulary: vocabulary,
        process: process)
    case .inspectorRead(let tab):
      result.parameters = try await inspectorParameters(
        tab: tab, window: window, vocabulary: vocabulary)
    case .inspectorSet(let change):
      result.parameters = [
        try await setInspector(change, window: window, vocabulary: vocabulary)
      ]
      result.changed = true
    case .pasteEffects(let request):
      result.paste = try await pasteEffects(
        request, window: window, vocabulary: vocabulary, process: process)
      result.project = try projectState(try finalCutLandmarks(in: window))
    case .effectParameters(let uid):
      guard let application = backend.applicationURL(bundleID: process.bundleID) else {
        throw ProAppsError.unavailable("Selected app edition is not installed")
      }
      result.effectParameters = try FCPEffectCatalog.parameters(application: application, uid: uid)
    case .closeLibrary(let library):
      _ = try await inForeground(vocabulary: vocabulary, process: process) {
        try await closeLibrary(library, window: window, vocabulary: vocabulary, process: process)
      }
      result.libraryClosed = true
    case .effectCatalog(let query):
      guard let application = backend.applicationURL(bundleID: process.bundleID) else {
        throw ProAppsError.unavailable("Selected app edition is not installed")
      }
      result.effects = try FCPEffectCatalog.scan(
        application: application, language: vocabulary.language, query: query)
    case .xmlExport(let request):
      let plan = try FCPExportPlan.make(
        request, home: home, language: vocabulary.language, localizations: folderLocalizations,
        extensions: FCPExportPlan.xmlExtensions)
      guard result.project.name == request.project else {
        throw ProAppsError.invalid(
          "The timeline shows a different project; open the requested project first")
      }
      result.xmlExport = try await exportXML(
        request, plan: plan, timeline: try requireTimeline(landmarks), vocabulary: vocabulary,
        process: process)
    }
    return result
  }

  // MARK: Discovery

  /// Detect the UI vocabulary, waiting while Final Cut Pro reports itself busy
  /// (AXError -25204), for example right after closing a library.
  func responsiveVocabulary(_ process: UIRunningProcess) async throws -> FCPVocabulary {
    let started = backend.monotonicSeconds()
    while true {
      do {
        return try finalCutVocabulary(process)
      } catch ProAppsError.unavailable(Self.busyReason)
        where backend.monotonicSeconds() - started < Self.finalCutBusyWait
      {
        try await backend.pause(seconds: Self.pollInterval)
      }
    }
  }

  func finalCutVocabulary(_ process: UIRunningProcess) throws -> FCPVocabulary {
    let bar = try menuContainer(path: [], process: process)
    let titles = try menuChildren(of: bar).compactMap { try string(.title, of: $0) }
    guard let vocabulary = FCPVocabulary.detect(menuBarTitles: titles) else {
      throw ProAppsError.unavailable("Unsupported Final Cut Pro UI language")
    }
    return vocabulary
  }

  func finalCutMainWindow(_ process: UIRunningProcess) throws -> UIHandle {
    let standard = try windows(of: process).filter {
      try string(.subrole, of: $0) == "AXStandardWindow"
    }
    let main =
      standard.count > 1 ? try standard.filter { try bool(.main, of: $0) == true } : standard
    guard main.count == 1 else {
      throw ProAppsError.unavailable("Final Cut Pro has no unique main window")
    }
    return main[0]
  }

  /// Breadth-first traversal that does not descend into large collections
  /// (tables, outlines, lists, the timeline) unless the root itself is one.
  func finalCutNodes(from root: UIHandle, maxDepth: Int, skipping: Set<String>) throws
    -> [FCPNode]
  {
    var found: [FCPNode] = []
    var queue: [(FCPNode, Int)] = [(FCPNode(handle: root, parent: nil, role: nil), 0)]
    while !queue.isEmpty, found.count < Self.maximumSearchNodes {
      let (node, depth) = queue.removeFirst()
      let role = try string(.role, of: node.handle)
      found.append(FCPNode(handle: node.handle, parent: node.parent, role: role))
      guard depth < maxDepth else { continue }
      if depth > 0, let role, skipping.contains(role) { continue }
      for child in try children(of: node.handle) {
        queue.append((FCPNode(handle: child, parent: node.handle, role: nil), depth + 1))
      }
    }
    return found
  }

  func finalCutLandmarks(in window: UIHandle) throws -> FCPLandmarks {
    var landmarks = FCPLandmarks()
    let prefix = Self.finalCutTimelineIdentifierPrefix
    for node in try finalCutNodes(
      from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles)
    {
      if node.role == "AXLayoutArea", try string(.subrole, of: node.handle) == "AXTimeline" {
        landmarks.timeline = node.handle
      }
      switch try string(.identifier, of: node.handle) {
      case prefix + "projectNamePopUpButton": landmarks.projectName = node.handle
      case prefix + "projectInfo": landmarks.projectInfo = node.handle
      default: break
      }
    }
    return landmarks
  }

  func requireTimeline(_ landmarks: FCPLandmarks) throws -> UIHandle {
    guard let timeline = landmarks.timeline else {
      throw ProAppsError.unavailable("No project is open in the Final Cut Pro timeline")
    }
    return timeline
  }

  func projectState(_ landmarks: FCPLandmarks) throws -> FCPProjectState {
    FCPProjectState(
      name: try landmarks.projectName.flatMap { try string(.title, of: $0) },
      duration: try landmarks.projectInfo.flatMap { try text(.value, of: $0) },
      playhead: try landmarks.timeline.flatMap { try playhead(in: $0) })
  }

  func playhead(in timeline: UIHandle) throws -> String? {
    try presentPlayheadIndicator(timeline).flatMap { try text(.value, of: $0) }
  }

  func presentPlayheadIndicator(_ timeline: UIHandle) throws -> UIHandle? {
    try children(of: timeline).first { try string(.role, of: $0) == "AXValueIndicator" }
  }

  /// The timeline's playhead element. Resolve it once per request: a long
  /// timeline has thousands of children to scan.
  func playheadIndicator(_ timeline: UIHandle) throws -> UIHandle {
    guard let indicator = try presentPlayheadIndicator(timeline) else {
      throw ProAppsError.unavailable("The timeline exposes no playhead")
    }
    return indicator
  }

  func timelineClips(_ timeline: UIHandle) throws -> [UIHandle] {
    try children(of: timeline).filter {
      try string(.role, of: $0) == "AXLayoutItem" && string(.subrole, of: $0) == "AXTimeline"
    }
  }

  func clipSummary(_ clip: UIHandle, index: Int) throws -> FCPClip {
    FCPClip(
      index: index, description: try string(.description, of: clip),
      start: try text(.valueDescription, of: clip), duration: try text(.value, of: clip),
      selected: try bool(.selected, of: clip))
  }

  /// Title, description and value strings of an element and its descendants.
  func finalCutTexts(_ handle: UIHandle, depth: Int) throws -> [String] {
    var texts = try [UIAttributeName.title, .description, .value].compactMap {
      try text($0, of: handle)
    }
    guard depth > 0 else { return texts }
    for child in try children(of: handle) {
      texts += try finalCutTexts(child, depth: depth - 1)
    }
    return texts
  }

  // MARK: Menus and waiting

  func finalCutMenuItem(_ path: [String], process: UIRunningProcess) throws -> UIHandle {
    let container = try menuContainer(path: Array(path.dropLast()), process: process)
    guard let title = path.last else { throw ProAppsError.invalid("Menu path is empty") }
    return try uniqueMenuItem(titled: title, in: container)
  }

  func pressFinalCutMenu(_ path: [String], process: UIRunningProcess) throws {
    let item = try finalCutMenuItem(path, process: process)
    try pressEnabled(item, name: path.joined(separator: " > "))
  }

  func pressEnabled(_ item: UIHandle, name: String) throws {
    guard try bool(.enabled, of: item) != false else {
      throw ProAppsError.unavailable(
        "Final Cut Pro disabled \(name); check the selection and focused panel")
    }
    try perform(.press, on: item)
  }

  /// Poll `probe` until it returns a value or `seconds` elapse.
  func finalCutWait<T>(
    _ seconds: Double, interval: Double = Self.pollInterval, _ probe: () throws -> T?
  ) async throws -> T? {
    let started = backend.monotonicSeconds()
    while true {
      try Task.checkCancellation()
      if let value = try probe() { return value }
      if backend.monotonicSeconds() - started >= seconds { return nil }
      try await backend.pause(seconds: interval)
    }
  }

  // MARK: Selection and projects

  func selectClips(_ references: [FCPClipReference], timeline: UIHandle) throws -> [FCPClip] {
    let indices = references.map(\.index)
    guard !references.isEmpty, Set(indices).count == indices.count else {
      throw ProAppsError.invalid("Select at least one clip, each index once")
    }
    let clips = try timelineClips(timeline)
    let handles = try references.map { reference -> UIHandle in
      guard clips.indices.contains(reference.index),
        try string(.description, of: clips[reference.index]) == reference.description
      else {
        throw ProAppsError.invalid(
          "Clip \(reference.index) no longer matches its description; read the timeline again")
      }
      return clips[reference.index]
    }
    try mappedBackend { try backend.set(.selectedChildren, to: .elements(handles), on: timeline) }
    let selected = try timelineClips(timeline).enumerated().filter {
      try bool(.selected, of: $0.element) == true
    }
    guard selected.map(\.offset) == indices.sorted() else {
      throw ProAppsError.unavailable("Final Cut Pro did not apply the requested selection")
    }
    return try selected.map { try clipSummary($0.element, index: $0.offset) }
  }

  func openProject(
    library: String?, event: String, project: String, window: UIHandle,
    vocabulary: FCPVocabulary, process: UIRunningProcess
  ) async throws {
    let (outline, row) = try eventRow(library: library, event: event, in: window)
    try mappedBackend { try backend.set(.selectedRows, to: .elements([row]), on: outline) }
    guard
      let (parent, item) = try await finalCutWait(
        Self.finalCutShortWait,
        {
          try projectItem(project, in: window)
        })
    else {
      throw ProAppsError.unavailable("The project is not shown in the browser filmstrip")
    }
    try mappedBackend { try backend.set(.selectedChildren, to: .elements([item]), on: parent) }
    // Clip > Open Clip follows keyboard focus, which may still be on the timeline.
    try mappedBackend { try backend.set(.focused, to: .bool(true), on: parent) }
    let before = try timelineIdentity(window)
    try pressFinalCutMenu(vocabulary.openClip, process: process)
    guard
      try await finalCutWait(
        Self.finalCutDialogWait,
        {
          try projectState(try finalCutLandmarks(in: window)).name == project ? true : nil
        }) != nil
    else {
      // Open Clip acted on the timeline instead (it opened a clip of the shown
      // project): step back so the user's timeline is left as it was.
      let after = try timelineIdentity(window)
      if after.project == before.project, after.clip != before.clip {
        try perform(.press, on: try timelineBackButton(window))
        throw ProAppsError.unavailable(
          "Open Clip opened a clip of the shown project instead; navigated back. Nothing was changed"
        )
      }
      throw ProAppsError.unavailable("Final Cut Pro did not open the project in the timeline")
    }
  }

  /// The timeline's project title and the opened clip (the pop-up's value).
  func timelineIdentity(_ window: UIHandle) throws -> (project: String?, clip: String?) {
    let button = try finalCutLandmarks(in: window).projectName
    return (
      try button.flatMap { try string(.title, of: $0) },
      try button.flatMap { try text(.value, of: $0) }
    )
  }

  func timelineBackButton(_ window: UIHandle) throws -> UIHandle {
    try uniqueNode(
      in: window, depth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles,
      name: "Timeline back button"
    ) {
      try string(.identifier, of: $0.handle)
        == Self.finalCutTimelineIdentifierPrefix + "timelineNavigationBackButton"
    }
  }

  /// The unique event row (disclosure level 1) named `event`, optionally inside
  /// the level-0 library row named `library`.
  func eventRow(library: String?, event: String, in window: UIHandle) throws
    -> (UIHandle, UIHandle)
  {
    let candidates = try eventRows(library: library, event: event, in: window)
    guard candidates.count == 1 else {
      throw ProAppsError.invalid(
        "Event \(event) matched \(candidates.count) library rows; name the library")
    }
    return candidates[0]
  }

  /// Every (outline, row) pair naming `event`, optionally inside `library`.
  func eventRows(library: String?, event: String, in window: UIHandle) throws
    -> [(UIHandle, UIHandle)]
  {
    var candidates: [(UIHandle, UIHandle)] = []
    let outlines = try finalCutNodes(
      from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles
    ).filter { $0.role == "AXOutline" }
    for outline in outlines {
      var inLibrary = library == nil
      for row in try children(of: outline.handle) where try string(.role, of: row) == "AXRow" {
        let texts = try finalCutTexts(row, depth: Self.textSearchDepth)
        guard let value = try read(.disclosureLevel, of: row), case .number(let level) = value
        else {
          continue
        }
        if level == 0, let library { inLibrary = texts.contains(library) }
        if level == 1, inLibrary, texts.contains(event) {
          candidates.append((outline.handle, row))
        }
      }
    }
    return candidates
  }

  /// A browser filmstrip item for `project`: a group whose title field shows its name.
  func projectItem(_ project: String, in window: UIHandle) throws -> (UIHandle, UIHandle)? {
    let groups = try finalCutNodes(
      from: window, maxDepth: Self.maximumSearchDepth, skipping: Self.finalCutSkippedRoles
    ).filter { node in
      guard node.role == "AXGroup", node.parent != nil,
        try string(.description, of: node.handle) == project
      else { return false }
      return try children(of: node.handle).contains {
        try string(.role, of: $0) == "AXTextField" && text(.value, of: $0) == project
      }
    }
    guard let item = groups.first, let parent = item.parent else { return nil }
    guard groups.count == 1 else {
      throw ProAppsError.invalid("More than one browser item is named \(project)")
    }
    return (parent, item.handle)
  }

  // MARK: Playhead

  /// Press one movement item and return the new playhead once it changes.
  func step(_ item: UIHandle, indicator: UIHandle) async throws -> FCPTimecode? {
    let before = try text(.value, of: indicator)
    try pressEnabled(item, name: "a Mark menu movement")
    return try await finalCutWait(Self.finalCutStepSettle, interval: Self.finalCutStepPoll) {
      let now = try text(.value, of: indicator)
      return now != before ? now.flatMap(FCPTimecode.init) : nil
    }
  }

  /// Exact seek without keyboard input: hop edit points that do not pass the
  /// target, then step single frames, reading the playhead back after each move.
  func seek(
    to target: FCPTimecode, maximumSteps: Int, timeline: UIHandle, vocabulary: FCPVocabulary,
    process: UIRunningProcess
  ) async throws -> Int {
    func item(_ move: FCPPlayheadMove) throws -> UIHandle {
      try finalCutMenuItem(try vocabulary.path(move), process: process)
    }
    let indicator = try playheadIndicator(timeline)
    guard var current = try text(.value, of: indicator).flatMap(FCPTimecode.init) else {
      throw ProAppsError.unavailable("The playhead timecode is not readable")
    }
    var steps = 0
    if target < current, let moved = try await step(try item(.start), indicator: indicator) {
      current = moved
      steps += 1
    }
    let nextEdit = try item(.nextEdit)
    while current < target, steps < maximumSteps,
      let moved = try await step(nextEdit, indicator: indicator)
    {
      steps += 1
      guard moved <= target else {
        // Stepping back returns to the last edit point that does not pass the target.
        if let back = try await step(try item(.previousEdit), indicator: indicator) {
          current = back
        } else {
          current = moved
        }
        steps += 1
        break
      }
      current = moved
    }
    let nextFrame = try item(.nextFrame)
    while current < target, steps < maximumSteps,
      let moved = try await step(nextFrame, indicator: indicator)
    {
      steps += 1
      current = moved
    }
    guard current == target else {
      throw ProAppsError.unavailable(
        "Playhead stopped at \(current) before \(target) after \(steps) steps (timeline end, step budget, or no edit point near the target)"
      )
    }
    return steps
  }

  // MARK: Export

  func exportProject(
    _ request: FCPExportRequest, plan: FCPExportPlan, timeline: UIHandle,
    vocabulary: FCPVocabulary, process: UIRunningProcess
  ) async throws -> FCPExportEvidence {
    let (evidence, restored) = try await inForeground(vocabulary: vocabulary, process: process) {
      try await share(
        request, plan: plan, timeline: timeline, vocabulary: vocabulary, process: process)
    }
    var result = evidence
    result.foregroundRestored = restored
    return result
  }

  /// Run `body` with Final Cut Pro active, then restore the previous frontmost
  /// app. On any failure (including cancellation) the share/export UI is closed
  /// and focus restored before the error is rethrown with the cleanup outcome.
  func inForeground<T>(
    vocabulary: FCPVocabulary, process: UIRunningProcess, _ body: () async throws -> T
  ) async throws -> (T, Bool) {
    let previous = backend.frontmostProcessID()
    let app = backend.applicationElement(pid: process.pid)
    guard backend.activate(pid: process.pid),
      try await finalCutWait(
        Self.finalCutShortWait, { try bool(.frontmost, of: app) == true ? true : nil })
        != nil
    else {
      let restored = try await restoreFrontmost(previous, process: process)
      throw ProAppsError.unavailable(
        "Final Cut Pro did not become active (previous app restored: \(restored))")
    }
    let value: T
    do {
      value = try await body()
    } catch {
      // Both cleanup steps run even if the other fails (for example after cancellation).
      var notes: [String] = []
      do {
        let closed = try await cancelExportUI(vocabulary: vocabulary, process: process)
        notes.append("export UI closed: \(closed)")
      } catch let cleanupError {
        notes.append("closing the export UI failed: \(cleanupError)")
      }
      do {
        let restored = try await restoreFrontmost(previous, process: process)
        notes.append("previous app restored: \(restored)")
      } catch let restoreError {
        notes.append("restoring the previous app failed: \(restoreError)")
      }
      let note = " (" + notes.joined(separator: "; ") + ")"
      switch error {
      case ProAppsError.invalid(let reason): throw ProAppsError.invalid(reason + note)
      case ProAppsError.unavailable(let reason): throw ProAppsError.unavailable(reason + note)
      default: throw error
      }
    }
    return (value, try await restoreFrontmost(previous, process: process))
  }

  func restoreFrontmost(_ previous: Int32?, process: UIRunningProcess) async throws -> Bool {
    guard let previous, previous != process.pid else { return true }
    let app = backend.applicationElement(pid: previous)
    guard backend.activate(pid: previous) else { return false }
    let restored = try await finalCutWait(Self.finalCutShortWait) {
      try bool(.frontmost, of: app) == true ? true : nil
    }
    return restored != nil
  }

  func exportDialog(process: UIRunningProcess) throws -> UIHandle? {
    try windows(of: process).first { try string(.subrole, of: $0) == "AXDialog" }
  }

  func share(
    _ request: FCPExportRequest, plan: FCPExportPlan, timeline: UIHandle,
    vocabulary: FCPVocabulary, process: UIRunningProcess
  ) async throws -> FCPExportEvidence {
    try mappedBackend { try backend.set(.focused, to: .bool(true), on: timeline) }
    let shareItem = try defaultShareItem(vocabulary: vocabulary, process: process)
    guard
      try await finalCutWait(
        Self.finalCutShortWait,
        {
          try bool(.enabled, of: shareItem) == true ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("Final Cut Pro kept Share disabled while active") }
    try perform(.press, on: shareItem)
    guard
      let dialog = try await finalCutWait(
        Self.finalCutDialogWait,
        {
          try exportDialog(process: process)
        })
    else { throw ProAppsError.unavailable("The share dialog did not open") }
    let nodes = try finalCutNodes(from: dialog, maxDepth: Self.textSearchDepth, skipping: [])
    guard
      try nodes.contains(where: {
        try $0.role == "AXTextField" && (text(.value, of: $0.handle)) == request.project
      })
    else { throw ProAppsError.invalid("The share dialog names a different item") }
    let info = try dialogInfo(dialog)
    let settings = try uniqueNode(in: dialog, depth: Self.textSearchDepth, name: "Settings tab") {
      try $0.role == "AXRadioButton" && (string(.title, of: $0.handle)) == vocabulary.settingsTab
    }
    try perform(.press, on: settings)
    guard
      try await finalCutWait(
        Self.finalCutShortWait,
        {
          try popup(after: vocabulary.actionLabel, in: dialog) != nil ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("The share settings did not appear") }
    if let format = request.format {
      try await choose(try vocabulary.title(format), after: vocabulary.formatLabel, in: dialog)
    }
    if let codec = request.codec {
      try await choose(codec, after: vocabulary.codecLabel, in: dialog)
    }
    try await choose(vocabulary.saveOnly, after: vocabulary.actionLabel, in: dialog)
    let chosen = try [vocabulary.formatLabel, vocabulary.codecLabel, vocabulary.actionLabel].map {
      try popup(after: $0, in: dialog).flatMap { try text(.value, of: $0) }
    }
    try perform(.press, on: try button(vocabulary.next, in: dialog))
    guard
      let sheet = try await finalCutWait(
        Self.finalCutDialogWait,
        {
          try children(of: dialog).first { try string(.role, of: $0) == "AXSheet" }
        })
    else { throw ProAppsError.unavailable("The Save panel did not open") }
    try await saveAs(plan, sheet: sheet, vocabulary: vocabulary)
    guard
      try await finalCutWait(
        Self.finalCutDialogWait,
        {
          try exportDialog(process: process) == nil ? true : nil
        }) != nil
    else {
      throw ProAppsError.unavailable("The Save panel stayed open; a confirmation may be waiting")
    }
    return FCPExportEvidence(
      outputPath: plan.outputPath, dialogInfo: info, format: chosen[0], codec: chosen[1],
      action: chosen[2], foregroundRestored: false)
  }

  /// The default Share destination (its title carries the default marker).
  func defaultShareItem(vocabulary: FCPVocabulary, process: UIRunningProcess) throws -> UIHandle {
    let container = try menuContainer(path: vocabulary.share, process: process)
    let items = try menuChildren(of: container).filter {
      try string(.title, of: $0)?.contains(vocabulary.defaultDestinationMarker) == true
    }
    guard items.count == 1 else {
      throw ProAppsError.unavailable("Share has \(items.count) default destinations")
    }
    return items[0]
  }

  /// Described static texts on the dialog's top level (size, rate, duration…).
  func dialogInfo(_ dialog: UIHandle) throws -> [String: String] {
    var info: [String: String] = [:]
    for child in try children(of: dialog) where try string(.role, of: child) == "AXStaticText" {
      guard info.count < Self.finalCutMaximumDialogInfo,
        let key = try string(.description, of: child), let value = try text(.value, of: child),
        !key.isEmpty, !value.isEmpty
      else { continue }
      info[key] = value
    }
    return info
  }

  func uniqueNode(
    in root: UIHandle, depth: Int, skipping: Set<String> = [], name: String,
    _ match: (FCPNode) throws -> Bool
  ) throws -> UIHandle {
    let found = try finalCutNodes(from: root, maxDepth: depth, skipping: skipping).filter(match)
    guard found.count == 1 else {
      throw ProAppsError.unavailable("\(name) matched \(found.count) elements")
    }
    return found[0].handle
  }

  func button(_ title: String, in root: UIHandle) throws -> UIHandle {
    try uniqueNode(
      in: root, depth: Self.textSearchDepth, skipping: ["AXBrowser", "AXOutline"],
      name: "Button \(title)"
    ) { try $0.role == "AXButton" && (string(.title, of: $0.handle)) == title }
  }

  /// The pop-up button that directly follows the static-text label `label`.
  func popup(after label: String, in root: UIHandle) throws -> UIHandle? {
    for node in try finalCutNodes(from: root, maxDepth: Self.textSearchDepth, skipping: []) {
      let siblings = try children(of: node.handle)
      for (offset, child) in siblings.enumerated().dropLast()
      where try string(.role, of: child) == "AXStaticText"
        && text(.value, of: child)?.trimmingCharacters(in: .whitespaces) == label
      {
        let next = siblings[offset + 1]
        return try string(.role, of: next) == "AXPopUpButton" ? next : nil
      }
    }
    return nil
  }

  /// Select the pop-up menu item titled `title`, verifying the button reads it back.
  func choose(_ title: String, after label: String, in root: UIHandle) async throws {
    guard let popup = try popup(after: label, in: root) else {
      throw ProAppsError.unavailable("The share settings have no \(label) control")
    }
    guard try text(.value, of: popup) != title else { return }
    try perform(.press, on: popup)
    guard
      let menu = try await finalCutWait(
        Self.finalCutShortWait,
        {
          try children(of: popup).first { try string(.role, of: $0) == "AXMenu" }
        })
    else { throw ProAppsError.unavailable("The \(label) menu did not open") }
    let items = try children(of: menu).filter { try string(.title, of: $0) == title }
    guard items.count == 1 else {
      throw ProAppsError.invalid("\(label) offers \(items.count) items titled \(title)")
    }
    try perform(.press, on: items[0])
    guard
      try await finalCutWait(
        Self.finalCutShortWait,
        {
          try text(.value, of: popup) == title ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("\(label) did not change to \(title)") }
  }

  /// Navigate the Save panel from the sidebar home item through the column
  /// browser, then name the file and press Save. Each step is read back through
  /// the location pop-up so a mismatched folder stops the export.
  func saveAs(_ plan: FCPExportPlan, sheet: UIHandle, vocabulary: FCPVocabulary) async throws {
    if try finalCutNodes(from: sheet, maxDepth: Self.textSearchDepth, skipping: []).contains(
      where: { $0.role == "AXBrowser" }) == false
    {
      let disclosure = try uniqueNode(in: sheet, depth: Self.textSearchDepth, name: "Disclosure") {
        $0.role == "AXDisclosureTriangle"
      }
      try perform(.press, on: disclosure)
    }
    guard
      let browser = try await finalCutWait(
        Self.finalCutShortWait,
        {
          try finalCutNodes(from: sheet, maxDepth: Self.textSearchDepth, skipping: [])
            .first { $0.role == "AXBrowser" }?.handle
        })
    else { throw ProAppsError.unavailable("The Save panel did not show its folder browser") }
    let outline = try uniqueNode(in: sheet, depth: Self.textSearchDepth, name: "Sidebar") {
      $0.role == "AXOutline"
    }
    let homeRows = try children(of: outline).filter {
      try string(.role, of: $0) == "AXRow"
        && finalCutTexts($0, depth: Self.textSearchDepth).contains(plan.homeDisplayName)
    }
    guard homeRows.count == 1 else {
      throw ProAppsError.unavailable("The Save panel sidebar has no unique home item")
    }
    if try bool(.selected, of: homeRows[0]) != true {
      try savePanelStep("selecting the home folder") {
        try backend.set(.selectedRows, to: .elements(homeRows), on: outline)
      }
    }
    // Column `level` lists the folder reached after `level` components. A column
    // that already selects the right folder is kept; a deeper leftover selection
    // in the destination column is cleared so the panel saves exactly there.
    for (level, component) in plan.components.enumerated() {
      let list = try await column(level, of: browser)
      guard try selectedNames(list) != [component] else { continue }
      let items = try children(of: list).filter { try itemName($0) == component }
      guard items.count == 1 else {
        throw ProAppsError.unavailable(
          "The Save panel shows \(items.count) folders named \(component)")
      }
      try savePanelStep("opening \(component)") {
        try backend.set(.selectedChildren, to: .elements(items), on: list)
      }
      guard
        try await finalCutWait(
          Self.finalCutShortWait,
          {
            try selectedNames(list) == [component] && columnLists(browser).count > level + 1
              ? true : nil
          }) != nil
      else { throw ProAppsError.unavailable("The Save panel did not open \(component)") }
    }
    let destination = try await column(plan.components.count, of: browser)
    if try !selectedNames(destination).isEmpty {
      try savePanelStep("clearing the destination selection") {
        try backend.set(.selectedChildren, to: .elements([]), on: destination)
      }
    }
    try await awaitLocation(plan.components.last ?? plan.homeDisplayName, sheet: sheet)
    let name = try uniqueNode(
      in: sheet, depth: Self.textSearchDepth, skipping: ["AXBrowser", "AXOutline"],
      name: "File name field"
    ) {
      try $0.role == "AXTextField" && (string(.subrole, of: $0.handle)) != "AXSearchField"
    }
    try mappedBackend {
      try backend.set(
        .value, to: .string(URL(fileURLWithPath: plan.outputPath).lastPathComponent), on: name)
    }
    guard try text(.value, of: name) == URL(fileURLWithPath: plan.outputPath).lastPathComponent
    else {
      throw ProAppsError.unavailable("The Save panel did not accept the file name")
    }
    try perform(.press, on: try button(vocabulary.save, in: sheet))
  }

  /// Run one Save panel write and name the step in any Accessibility failure.
  func savePanelStep(_ step: String, _ body: () throws -> Void) throws {
    do {
      try body()
    } catch let error as UIBackendError {
      throw ProAppsError.unavailable("Save panel step failed while \(step): \(Self.mapped(error))")
    }
  }

  func awaitLocation(_ displayName: String, sheet: UIHandle) async throws {
    guard
      try await finalCutWait(
        Self.finalCutShortWait,
        {
          try finalCutNodes(from: sheet, maxDepth: Self.textSearchDepth, skipping: ["AXBrowser"])
            .contains {
              try $0.role == "AXPopUpButton" && (text(.value, of: $0.handle)) == displayName
            } ? true : nil
        }) != nil
    else { throw ProAppsError.unavailable("The Save panel did not move to \(displayName)") }
  }

  /// Wait for column `index` of the Save panel browser and return its list.
  func column(_ index: Int, of browser: UIHandle) async throws -> UIHandle {
    guard
      let list = try await finalCutWait(
        Self.finalCutShortWait,
        {
          let lists = try columnLists(browser)
          return lists.count > index ? lists[index] : nil
        })
    else { throw ProAppsError.unavailable("The Save panel column browser is not readable") }
    return list
  }

  /// Names of the items selected in one browser column.
  func selectedNames(_ list: UIHandle) throws -> [String] {
    guard let value = try read(.selectedChildren, of: list) else { return [] }
    let selected = UIAutomation.handles(value)
    return try selected.compactMap(itemName)
  }

  /// The visible name of a Save panel browser item (its text field value).
  func itemName(_ item: UIHandle) throws -> String? {
    try children(of: item).first { try string(.role, of: $0) == "AXTextField" }
      .flatMap { try text(.value, of: $0) }
  }

  /// The list of every column currently shown by the Save panel's column browser.
  func columnLists(_ browser: UIHandle) throws -> [UIHandle] {
    let areas = try children(of: browser).filter { try string(.role, of: $0) == "AXScrollArea" }
    let columns = try areas.flatMap { area in
      try children(of: area).filter { try string(.role, of: $0) == "AXScrollArea" }
    }
    return try columns.compactMap { column in
      try children(of: column).first { try string(.role, of: $0) == "AXList" }
    }
  }

  /// Close a share dialog left open by a failed export: the Save sheet first,
  /// then the dialog. Returns whether no dialog remains.
  func cancelExportUI(vocabulary: FCPVocabulary, process: UIRunningProcess) async throws -> Bool {
    guard let dialog = try exportDialog(process: process) else { return true }
    if let sheet = try children(of: dialog).first(where: { try string(.role, of: $0) == "AXSheet" })
    {
      try perform(.press, on: try button(vocabulary.cancel, in: sheet))
      _ = try await finalCutWait(Self.finalCutShortWait) {
        try children(of: dialog).contains { try string(.role, of: $0) == "AXSheet" } ? nil : true
      }
    }
    try perform(.press, on: try button(vocabulary.cancel, in: dialog))
    let closed = try await finalCutWait(Self.finalCutShortWait) {
      try exportDialog(process: process) == nil ? true : nil
    }
    return closed != nil
  }
}
