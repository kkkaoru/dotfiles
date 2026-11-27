import CryptoKit
import Foundation

/// Turns ML Program text into a compiled Core ML model, with the vendored schema's C++ encoder
/// (`protoc`) and Xcode's compiler (`coremlc`) as the only tools — no Python anywhere.
///
/// The result is cached by the hash of the text, so the second start of the server loads an
/// `.mlmodelc` instead of compiling again. Compiling the whole decoder takes minutes; loading it
/// takes milliseconds.
public struct ProgramCompiler: Sendable {
    /// The vendored `.proto` files, which are schema data rather than code.
    public let protoPath: URL
    /// Where the compiled models live.
    public let cache: URL

    public init(protoPath: URL = ProgramCompiler.defaultProtoPath, cache: URL) {
        self.protoPath = protoPath
        self.cache = cache
    }

    /// `PLAMO_COREML_PROTO`, else the checkout this file was built from. An installed copy of the
    /// server keeps the repository beside it, so the second form works there too.
    public static var defaultProtoPath: URL {
        if let path = ProcessInfo.processInfo.environment["PLAMO_COREML_PROTO"] {
            return URL(fileURLWithPath: path)
        }
        return URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Vendor/CoreMLProto")
    }

    /// Compiles `text` and returns the `.mlmodelc`, reusing an earlier build of the same text.
    public func compile(_ text: String, name: String) throws -> URL {
        let digest = SHA256.hash(data: Data(text.utf8))
            .map { String(format: "%02x", $0) }
            .joined()
            .prefix(16)
        let built = cache.appendingPathComponent("\(name)-\(digest).mlmodelc")
        if FileManager.default.fileExists(atPath: built.path) {
            return built
        }
        try FileManager.default.createDirectory(at: cache, withIntermediateDirectories: true)

        let work = cache.appendingPathComponent("build-\(name)-\(digest)")
        try? FileManager.default.removeItem(at: work)
        try FileManager.default.createDirectory(at: work, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: work) }

        let textURL = work.appendingPathComponent("model.prototxt")
        try Data(text.utf8).write(to: textURL)
        let mlmodel = work.appendingPathComponent("model.mlmodel")
        try run(
            "/usr/bin/env",
            [
                "protoc", "--proto_path=\(protoPath.path)", "--encode=CoreML.Specification.Model",
                protoPath.appendingPathComponent("Model.proto").path,
            ], input: textURL, output: mlmodel)

        let data = work.appendingPathComponent("model.mlpackage/Data/com.apple.CoreML")
        try FileManager.default.createDirectory(at: data, withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: mlmodel, to: data.appendingPathComponent("model.mlmodel"))
        try Data(Self.manifest.utf8).write(to: work.appendingPathComponent("model.mlpackage/Manifest.json"))

        try run("/usr/bin/xcrun", ["coremlc", "compile", "model.mlpackage", "."], cwd: work)
        try FileManager.default.moveItem(at: work.appendingPathComponent("model.mlmodelc"), to: built)
        return built
    }

    /// A manifest with one model entry; `coremlc` reads the identifier but only needs it to match.
    private static let manifest = """
        {"fileFormatVersion":"1.0.0","itemInfoEntries":\
        {"5B1F0C2A-0000-4000-8000-000000000001":{"author":"com.apple.CoreML","description":"plamo",\
        "name":"model.mlmodel","path":"com.apple.CoreML/model.mlmodel"}},\
        "rootModelIdentifier":"5B1F0C2A-0000-4000-8000-000000000001"}
        """

    private func run(
        _ tool: String, _ arguments: [String], input: URL? = nil, output: URL? = nil, cwd: URL? = nil
    ) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        process.currentDirectoryURL = cwd
        if let input { process.standardInput = try FileHandle(forReadingFrom: input) }
        if let output {
            FileManager.default.createFile(atPath: output.path, contents: nil)
            process.standardOutput = try FileHandle(forWritingTo: output)
        }
        let errors = Pipe()
        process.standardError = errors
        try process.run()
        let message = errors.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let text = String(decoding: message, as: UTF8.self)
                .split(separator: "\n").suffix(3).joined(separator: "\n")
            throw PlamoError.coreMLTool(
                tool: (arguments.first == "coremlc" ? "coremlc" : "protoc"), message: text)
        }
    }
}
