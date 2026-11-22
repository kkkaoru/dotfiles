import Foundation

/// The six things the binary can do. Parsing rejects anything else instead of silently serving.
public enum Command: String, CaseIterable, Sendable {
    case tokenize
    case load
    case generate
    case serve
    case debug
    case bench
}

/// A fully validated command line: every value is already the type the caller needs, so nothing
/// downstream has to parse or guess a default.
public struct Invocation: Sendable {
    public static let defaultPort: UInt16 = 18555
    public static let defaultIdleSeconds: Double = 600
    public static let defaultMaxTokens = 64

    public let command: Command
    public let modelDirectory: URL
    public let tokenizerBlob: URL
    public let port: UInt16
    /// The Unix socket the server also listens on; nil when only the loopback port is used.
    public let socketPath: String?
    public let idleSeconds: Double
    public let maxTokens: Int
    public let prompt: String

    public init(
        command: Command,
        modelDirectory: URL,
        tokenizerBlob: URL,
        port: UInt16 = Invocation.defaultPort,
        socketPath: String? = nil,
        idleSeconds: Double = Invocation.defaultIdleSeconds,
        maxTokens: Int = Invocation.defaultMaxTokens,
        prompt: String = ""
    ) {
        self.command = command
        self.modelDirectory = modelDirectory
        self.tokenizerBlob = tokenizerBlob
        self.port = port
        self.socketPath = socketPath
        self.idleSeconds = idleSeconds
        self.maxTokens = maxTokens
        self.prompt = prompt
    }

    /// `plamo-swift <command> --model <dir> [--tokenizer <blob>] [--port <port>] [--max-tokens <n>]`
    /// plus any positional words, which become the prompt. `--tokenizer` defaults to the blob the
    /// setup script writes beside the weights, so the mods' argv needs nothing else.
    public init(arguments: [String]) throws {
        var values = [String: String]()
        var positional = [String]()
        var command: String?
        var rest = arguments[...]
        while let argument = rest.popFirst() {
            if argument.hasPrefix("--") {
                let name = String(argument.dropFirst(2))
                if let next = rest.first, !next.hasPrefix("--") {
                    values[name] = next
                    rest = rest.dropFirst()
                } else {
                    values[name] = "true"
                }
            } else if command == nil {
                command = argument
            } else {
                positional.append(argument)
            }
        }

        guard let name = command else { throw PlamoError.missingCommand }
        guard let command = Command(rawValue: name) else { throw PlamoError.unknownCommand(name) }
        guard let model = values["model"] else { throw PlamoError.missingOption(name: "model") }
        let modelDirectory = URL(fileURLWithPath: model)
        let blob =
            values["tokenizer"] ?? modelDirectory.appendingPathComponent("tokenizer.bin").path
        self.init(
            command: command,
            modelDirectory: modelDirectory,
            tokenizerBlob: URL(fileURLWithPath: blob),
            port: try Invocation.port(values["port"]),
            socketPath: try Invocation.socket(values["socket"]),
            idleSeconds: try Invocation.positive(values["idle-seconds"], name: "idle-seconds")
                ?? Invocation.defaultIdleSeconds,
            maxTokens: try Invocation.count(values["max-tokens"], name: "max-tokens"),
            // `--prompt` used to land in `values` and never be read, so `generate`, `debug` and
            // `bench` ran on an empty prompt and printed the model's answer to nothing.
            prompt: ([values["prompt"]] + positional).compactMap { $0 }.joined(separator: " "))
    }

    private static func port(_ raw: String?) throws -> UInt16 {
        guard let raw else { return Invocation.defaultPort }
        guard let port = UInt16(raw) else {
            throw PlamoError.invalidOption(name: "port", value: raw)
        }
        return port
    }

    private static func socket(_ raw: String?) throws -> String? {
        guard let raw else { return nil }
        guard raw.hasPrefix("/") else {
            throw PlamoError.invalidOption(name: "socket", value: raw)
        }
        return raw
    }

    private static func positive(_ raw: String?, name: String) throws -> Double? {
        guard let raw else { return nil }
        guard let value = Double(raw), value > 0 else {
            throw PlamoError.invalidOption(name: name, value: raw)
        }
        return value
    }

    private static func count(_ raw: String?, name: String) throws -> Int {
        guard let raw else { return Invocation.defaultMaxTokens }
        guard let value = Int(raw), value > 0 else {
            throw PlamoError.invalidOption(name: name, value: raw)
        }
        return value
    }
}
