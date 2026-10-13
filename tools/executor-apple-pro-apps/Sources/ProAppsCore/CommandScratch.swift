import Foundation

/// Explicit host scratch overrides Foundation's process-specific temporary lookup.
enum CommandScratch {
  static func directory(environment: [String: String]) throws -> URL {
    guard let path = environment["TMPDIR"] else { return FileManager.default.temporaryDirectory }
    // macOS sets TMPDIR with a trailing separator; compare the normalized form.
    let declared = URL(fileURLWithPath: path).path
    let directory = try Files.absolute(path)
    guard directory.path == declared,
      directory.resolvingSymlinksInPath().path == directory.path,
      try directory.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true
    else {
      throw ProAppsError.invalid("TMPDIR must be an existing canonical non-symlink directory")
    }
    return directory
  }
}
