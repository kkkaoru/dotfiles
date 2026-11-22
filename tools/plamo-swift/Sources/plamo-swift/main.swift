import Dispatch
import Foundation
import MLX
import PlamoCore

func report(_ message: String) {
    FileHandle.standardError.write(Data("plamo-swift: \(message)\n".utf8))
}

func makeRuntime(_ invocation: Invocation) throws -> ModelRuntime {
    try ModelRuntime(
        modelDirectory: invocation.modelDirectory, tokenizerBlob: invocation.tokenizerBlob)
}

func loadReport(loaded: Double, warm: Double) -> String {
    String(
        format: "load %.3f s (tokenizer+weights %.3f, warm-up %.3f)", loaded + warm, loaded, warm)
}

func run(_ invocation: Invocation) async throws {
    switch invocation.command {
    case .tokenize:
        let meta = invocation.tokenizerBlob.deletingPathExtension().appendingPathExtension("json")
        let tokenizer = try PlamoTokenizer(blob: invocation.tokenizerBlob, meta: meta)
        print(tokenizer.encode(invocation.prompt).map(String.init).joined(separator: " "))

    case .load:
        let started = Date()
        let runtime = try makeRuntime(invocation)
        let loaded = Date().timeIntervalSince(started)
        let warm = try await runtime.warmUp()
        print(loadReport(loaded: loaded, warm: warm))

    case .debug:
        let runtime = try makeRuntime(invocation)
        _ = try await runtime.warmUp()
        let result = try await runtime.stages(prompt: invocation.prompt)
        for (stage, values) in result.stages {
            let norm = sqrt(values.reduce(0) { $0 + $1 * $1 })
            let head = values.prefix(3).map { String(format: "%.4f", $0) }.joined(separator: " ")
            print("\(stage) norm \(String(format: "%.4f", norm)) \(head)")
        }
        let top = result.top.enumerated().sorted { $0.element > $1.element }.prefix(5)
        print("prompt ids: \(result.ids)")
        print("eos: \(await runtime.eosTokenIDs)")
        print("top: " + top.map { "\($0.offset):\(String(format: "%.3f", $0.element))" }
            .joined(separator: " "))

    case .generate:
        let started = Date()
        let runtime = try makeRuntime(invocation)
        let loaded = Date().timeIntervalSince(started)
        _ = try await runtime.warmUp()
        let completion = try await runtime.complete(
            CompletionRequest(prompt: invocation.prompt, maxTokens: invocation.maxTokens, stop: []))
        // The parity script skips the first two lines and compares the rest.
        print("ids \(await runtime.encode(invocation.prompt))")
        print(
            String(
                format: "load %.3f s, %d tokens in %.3f s", loaded, completion.tokens,
                completion.seconds))
        print(completion.text)

    case .serve:
        let started = Date()
        let runtime = try makeRuntime(invocation)
        let loaded = Date().timeIntervalSince(started)
        let warm = try await runtime.warmUp()
        print(loadReport(loaded: loaded, warm: warm))
        let server = PlamoServer(
            router: Router(provider: runtime, defaultMaxTokens: invocation.maxTokens),
            configuration: .init(
                port: invocation.port,
                socketPath: invocation.socketPath,
                idleSeconds: invocation.idleSeconds))
        let endpoints = try await server.start()
        print("ready on \(endpoints)")
        // The listeners own their queues; suspending this task forever is what keeps the process up.
        // dispatchMain() here traps: async main already runs on the main queue.
        await withCheckedContinuation { (_: CheckedContinuation<Void, Never>) in }
    }
}

let invocation: Invocation
do {
    invocation = try Invocation(arguments: Array(CommandLine.arguments.dropFirst()))
} catch {
    report("\(error)")
    exit(1)
}
MLXMemory.configure()
do {
    try await run(invocation)
} catch {
    report("\(error)")
    exit(1)
}
