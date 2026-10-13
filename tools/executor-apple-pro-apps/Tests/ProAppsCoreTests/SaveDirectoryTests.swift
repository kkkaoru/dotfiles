import Foundation
import Testing

@testable import ProAppsCore

@MainActor
struct SaveDirectoryTests {
  enum Failure: String, CaseIterable {
    case rejected, unmoved, noMenu, wrongAncestry, openMenu, collision, cleanupFailure,
      oversizedMenu, missingLocation, missingTitle
  }

  @Test func ancestryUsesApplicationLanguageForMarkedSystemFolders() throws {
    let paths = try FinalCutAutomationTests().exportHome()
    defer {
      do { try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent()) } catch {
        Issue.record(error)
      }
    }
    let japanese = try UIAutomation<FakeUIBackend>.saveDirectoryAncestry(
      paths.output, language: "ja", localizations: paths.tables)
    let english = try UIAutomation<FakeUIBackend>.saveDirectoryAncestry(
      paths.output, language: "en", localizations: paths.tables)
    #expect(Array(japanese.prefix(3)) == ["out", "ムービー", "user"])
    #expect(Array(english.prefix(3)) == ["out", "Movies", "user"])
  }

  func prepare(_ world: FakeFinalCut, directory: URL, failure: Failure? = nil) throws {
    world.openSheet()
    world.backend.attributes[312]?[.identifier] = .string("saveAsNameTextField")
    world.backend.attributes[311]?[.identifier] = .string("where popup")
    world.backend.attributes[315]?[.identifier] = .string("OKButton")
    world.backend.attributes[315]?[.enabled] = .bool(true)
    if failure == .rejected { world.backend.ignoresWrites = true }
    let ancestry = try UIAutomation<FakeUIBackend>.saveDirectoryAncestry(directory, language: "ja")
    if failure != .unmoved {
      world.backend.attributes[311]?[.value] = .string(ancestry.first ?? "")
    }
    if failure == .missingLocation { world.backend.attributes[311]?[.value] = nil }
    let previous = world.backend.onPerform
    world.backend.onPerform = { backend, action, handle in
      previous(backend, action, handle)
      if handle == 311 && action == "AXPress" && failure != .noMenu {
        backend.attributes[311]?[.children] = .elements([900])
        let titles =
          failure == .wrongAncestry || failure == .cleanupFailure
          ? [ancestry.first ?? "", "different"] : ancestry
        backend.attributes[900] = [
          .role: .string("AXMenu"), .children: .elements(titles.indices.map { 901 + $0 }),
        ]
        for (index, title) in titles.enumerated() {
          backend.attributes[901 + index] = [.title: .string(title)]
        }
        if failure == .cleanupFailure { backend.performFailure = .accessibility(-25205) }
        if failure == .missingTitle { backend.attributes[901]?[.title] = nil }
        if failure == .oversizedMenu {
          backend.attributes[900]?[.children] = .elements(Array(10000...15000))
        }
      }
      if handle == 900 && action == "AXCancel" && failure != .openMenu {
        backend.attributes[311]?[.children] = .elements([])
      }
    }
  }

  @Test func currentDirectoryNeedsCompleteAncestryBeforeSave() async throws {
    let paths = try FinalCutAutomationTests().exportHome()
    defer {
      do { try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent()) } catch {
        Issue.record(error)
      }
    }
    let world = FakeFinalCut(homeName: "user")
    try prepare(world, directory: paths.output)
    let ui = UIAutomation(backend: world.backend)
    let plan = FCPExportPlan(
      outputPath: paths.output.appendingPathComponent("new.mov").path,
      homeDisplayName: "user", components: [], usesHomeColumns: false)
    try await ui.saveAs(plan, sheet: 300, vocabulary: .japanese)
    #expect(world.savedName == "new.mov")
    #expect(world.backend.performed == ["AXPress@311", "AXCancel@900", "AXPress@315"])
    #expect(world.backend.activated.isEmpty)
  }

  @Test(arguments: Failure.allCases)
  func neverSavesWhenDestinationIsUnproven(_ failure: Failure) async throws {
    let paths = try FinalCutAutomationTests().exportHome()
    defer {
      do { try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent()) } catch {
        Issue.record(error)
      }
    }
    let world = FakeFinalCut(homeName: "user")
    try prepare(world, directory: paths.output, failure: failure)
    let path = paths.output.appendingPathComponent("new.mov")
    if failure == .collision { try Data().write(to: path) }
    let plan = FCPExportPlan(
      outputPath: path.path, homeDisplayName: "user", components: [],
      usesHomeColumns: false)
    await #expect(throws: ProAppsError.self) {
      try await UIAutomation(backend: world.backend).saveAs(plan, sheet: 300, vocabulary: .japanese)
    }
    #expect(world.savedName == nil)
    if failure == .wrongAncestry || failure == .oversizedMenu {
      #expect(world.backend.attributes[311]?[.children] == .elements([]))
    }
  }

  @Test func verifiedCurrentDirectoryNeverSelectsHomeOrNeedsColumns() async throws {
    let paths = try FinalCutAutomationTests().exportHome()
    defer {
      do { try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent()) } catch {
        Issue.record(error)
      }
    }
    let world = FakeFinalCut(homeName: "user")
    try prepare(world, directory: paths.output)
    world.backend.attributes[301]?[.children] = .elements([302, 305, 311, 312, 313, 314, 315])
    world.backend.attributes[304]?[.selected] = .bool(true)
    let plan = FCPExportPlan(
      outputPath: paths.output.appendingPathComponent("new.mov").path,
      homeDisplayName: "user", components: ["ムービー", "out"])
    try await UIAutomation(backend: world.backend).saveAs(plan, sheet: 300, vocabulary: .japanese)
    #expect(world.savedName == "new.mov")
  }

  @Test func externalCanonicalDestinationAndDanglingCollision() throws {
    let paths = try FinalCutAutomationTests().exportHome()
    defer {
      do { try FileManager.default.removeItem(at: paths.home.deletingLastPathComponent()) } catch {
        Issue.record(error)
      }
    }
    let destination = paths.home.deletingLastPathComponent()
    let request = FCPExportRequest(
      project: "P", directory: destination.path,
      fileName: "new.mov", allowForeground: true)
    let plan = try FCPExportPlan.make(
      request, home: paths.home, language: "en",
      localizations: paths.tables)
    #expect(!plan.usesHomeColumns)
    let path = destination.appendingPathComponent("new.mov")
    try FileManager.default.createSymbolicLink(
      atPath: path.path,
      withDestinationPath: destination.appendingPathComponent("missing").path)
    #expect(throws: ProAppsError.self) { try FCPExportPlan.requireNewDestination(path) }
    #expect(throws: ProAppsError.self) {
      try FCPExportPlan.requireNewDestination(destination.appendingPathComponent("missing/new.mov"))
    }
  }
}
