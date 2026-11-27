import CoreML
import Foundation
import Metal

/// What one machine offers, and what each engine measured on it. A Neural Engine's core count
/// differs per device and Apple does not publish it, so the numbers are measured once per
/// environment and kept beside the environment they belong to: a measurement from another machine,
/// or from another OS build on the same machine, is never reused.
public struct ExecutionEnvironment: Codable, Sendable, Hashable {
    /// `hw.model`, e.g. `Mac15,6`.
    public let model: String
    /// `kern.osversion`, the build, which changes with every macOS update.
    public let os: String
    /// `machdep.cpu.brand_string`, e.g. `Apple M3 Pro`.
    public let chip: String
    /// The Metal device's name, which is the GPU the shaders actually run on.
    public let gpu: String
    /// Whether a Neural Engine compute device exists at all.
    public let neuralEngine: Bool

    public init(model: String, os: String, chip: String, gpu: String, neuralEngine: Bool) {
        self.model = model
        self.os = os
        self.chip = chip
        self.gpu = gpu
        self.neuralEngine = neuralEngine
    }

    /// One line that changes when the machine or its OS does.
    public var fingerprint: String {
        "\(model)|\(os)|\(chip)|\(gpu)|\(neuralEngine ? "ane" : "no-ane")"
    }
}

/// Reads the current environment. Every field is a probe of the running machine, so a test that
/// wants another environment builds one by hand instead.
public enum EnvironmentProbe {
    /// Probes the running machine.
    public static func current() -> ExecutionEnvironment {
        ExecutionEnvironment(
            model: sysctl("hw.model") ?? "unknown",
            os: sysctl("kern.osversion") ?? "unknown",
            chip: sysctl("machdep.cpu.brand_string") ?? "unknown",
            gpu: MTLCreateSystemDefaultDevice()?.name ?? "none",
            neuralEngine: hasNeuralEngine
        )
    }

    /// The ANE is a compute device Core ML lists when the machine has one.
    internal static var hasNeuralEngine: Bool {
        MLComputeDevice.allComputeDevices.contains { device in
            if case .neuralEngine = device { true } else { false }
        }
    }

    internal static func sysctl(_ name: String) -> String? {
        var size = 0
        guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return nil }
        var buffer = [CChar](repeating: 0, count: size)
        guard sysctlbyname(name, &buffer, &size, nil, 0) == 0 else { return nil }
        let bytes = buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }
        return String(bytes: bytes, encoding: .utf8)
    }
}

/// One measured cost of one engine in one environment.
public struct EngineMeasurement: Codable, Sendable, Equatable {
    /// `coreml`, `mps`, or whatever else serves a step.
    public let engine: String
    /// Milliseconds for one decode step at a fixed cache length.
    public let millisecondsPerStep: Double
    public let measuredAt: Date

    public init(engine: String, millisecondsPerStep: Double, measuredAt: Date = Date()) {
        self.engine = engine
        self.millisecondsPerStep = millisecondsPerStep
        self.measuredAt = measuredAt
    }
}

/// Measurements per environment fingerprint.
public struct DiagnosticsCache: Codable, Sendable, Equatable {
    public struct Entry: Codable, Sendable, Equatable {
        public var environment: ExecutionEnvironment
        /// Keyed by engine name, so a new engine is one entry and an old one keeps its history.
        public var measurements: [String: EngineMeasurement]

        public init(
            environment: ExecutionEnvironment, measurements: [String: EngineMeasurement] = [:]
        ) {
            self.environment = environment
            self.measurements = measurements
        }
    }

    public var entries: [String: Entry]

    public init(entries: [String: Entry] = [:]) {
        self.entries = entries
    }
}

/// The diagnostics file: one JSON document beside the other caches, so a machine measures itself
/// once instead of on every launch.
public struct DiagnosticsStore: Sendable {
    public let file: URL

    public init(file: URL = DiagnosticsStore.defaultFile) {
        self.file = file
    }

    /// `~/Library/Caches/plamo-swift/diagnostics.json`.
    public static var defaultFile: URL {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
        let base = caches ?? FileManager.default.temporaryDirectory
        return base.appendingPathComponent("plamo-swift/diagnostics.json")
    }

    /// The cache on disk, or an empty one when the file is missing or unreadable: a corrupt cache
    /// costs one measurement, never a failed request.
    public func load() -> DiagnosticsCache {
        let decoder = JSONDecoder()
        // Must match `save`, or every read silently starts from empty.
        decoder.dateDecodingStrategy = .iso8601
        guard let data = try? Data(contentsOf: file),
            let cache = try? decoder.decode(DiagnosticsCache.self, from: data)
        else { return DiagnosticsCache() }
        return cache
    }

    public func save(_ cache: DiagnosticsCache) throws {
        try FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        try encoder.encode(cache).write(to: file, options: .atomic)
    }

    /// Records one measurement under its environment, replacing an earlier number from the same
    /// engine and keeping every other environment untouched.
    public func record(_ measurement: EngineMeasurement, environment: ExecutionEnvironment) throws {
        var cache = load()
        var entry =
            cache.entries[environment.fingerprint]
            ?? DiagnosticsCache.Entry(environment: environment)
        entry.measurements[measurement.engine] = measurement
        cache.entries[environment.fingerprint] = entry
        try save(cache)
    }

    /// The stored measurement for `engine`, or nil when this environment has not measured it.
    public func measurement(
        for engine: String, environment: ExecutionEnvironment
    ) -> EngineMeasurement? {
        load().entries[environment.fingerprint]?.measurements[engine]
    }

    /// The fastest engine among `candidates` that this environment has measured, or nil when it has
    /// measured none of them. The caller decides what to do with nil, usually measuring.
    public func fastest(
        of candidates: [String], environment: ExecutionEnvironment
    ) -> EngineMeasurement? {
        let measurements = load().entries[environment.fingerprint]?.measurements ?? [:]
        return candidates.compactMap { measurements[$0] }
            .min { $0.millisecondsPerStep < $1.millisecondsPerStep }
    }
}
