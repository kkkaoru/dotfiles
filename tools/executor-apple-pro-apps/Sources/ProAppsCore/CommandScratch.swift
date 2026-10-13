import Foundation

/// Explicit host scratch overrides Foundation's process-specific temporary lookup.
enum CommandScratch {
  static func directory(environment: [String: String]) throws -> URL {
    guard let path = environment["TMPDIR"] else { return FileManager.default.temporaryDirectory }
    let directory = try Files.absolute(path)
    guard directory.path == path,
      directory.resolvingSymlinksInPath().path == path,
      try directory.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true
    else {
      throw ProAppsError.invalid("TMPDIR must be an existing canonical non-symlink directory")
    }
    return directory
  }
}
