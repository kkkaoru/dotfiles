import Dispatch
import Foundation
import MLX
import PlamoCore

internal func report(_ message: String) {
    FileHandle.standardError.write(Data("plamo-swift: \(message)\n".utf8))
}

internal func makeRuntime(_ invocation: Invocation) throws -> ModelRuntime {
    try ModelRuntime(
        modelDirectory: invocation.modelDirectory, tokenizerBlob: invocation.tokenizerBlob)
}

internal func loadReport(loaded: Double, warm: Double) -> String {
    String(
        format: "load %.3f s (tokenizer+weights %.3f, warm-up %.3f)", loaded + warm, loaded, warm)
}

/// The `debug` command: the same per-stage numbers the Python server prints for one prompt.
internal func debugReport(_ runtime: ModelRuntime, prompt: String) async throws {
    let result = try await runtime.stages(prompt: prompt)
    for (stage, values) in result.stages {
        let norm = sqrt(values.reduce(0) { $0 + $1 * $1 })
        let head = values.prefix(3).map { String(format: "%.4f", $0) }.joined(separator: " ")
        print("\(stage) norm \(String(format: "%.4f", norm)) \(head)")
    }
    let top = result.top.sorted(by: >).prefix(5)
    print("prompt ids: \(result.ids)")
    print("eos: \(await runtime.eosTokenIDs)")
    print(
        "top: "
            + top.enumerated().map { "\($0.offset):\(String(format: "%.3f", $0.element))" }
            .joined(separator: " "))
}

/// The `bench` command: prefill at the prompt and at 500 tokens, then one decode loop.
internal func benchmark(_ runtime: ModelRuntime, prompt: String) async throws {
    let ids = await runtime.encode(prompt)
    guard !ids.isEmpty else { throw PlamoError.invalidRequest(reason: "empty prompt") }
    let long = Array(Array(repeating: ids, count: 500 / ids.count + 1).joined().prefix(500))
    for padded in [ids, long] {
        // One forward pass now builds at most two tables (full and sliding), not one per layer.
        let maskStarted = Date()
        _ = try Mask.causal(queries: padded.count, keys: padded.count, window: nil)
        _ = try Mask.causal(
            queries: padded.count, keys: min(padded.count, await runtime.windowSize),
            window: await runtime.windowSize)
        let mask = Date().timeIntervalSince(maskStarted)
        let timing = try await runtime.timePrefill(padded)
        print(
            String(
                format: "prefill %4d tokens: build %6.2f ms, eval %7.2f ms, masks %6.3f ms",
                timing.tokens, timing.build * 1_000, timing.eval * 1_000, mask * 1_000))
    }
    // One decode loop per process: two in a row segfaults inside MLX, and the context length is
    // already the caller's prompt, so a long prompt measures the long-context step.
    let decode = try await runtime.timeDecode(prompt: ids, steps: 32)
    print(
        String(
            format: "decode per step at %d tokens: build %6.2f ms, eval %6.2f ms", ids.count,
            decode.build * 1_000, decode.eval * 1_000))
}

internal func run(_ invocation: Invocation) async throws {
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
        try await debugReport(runtime, prompt: invocation.prompt)

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

    case .bench:
        let started = Date()
        let runtime = try makeRuntime(invocation)
        let loaded = Date().timeIntervalSince(started)
        let warm = try await runtime.warmUp()
        print(loadReport(loaded: loaded, warm: warm))
        try await benchmark(runtime, prompt: invocation.prompt)

    case .serve:
        let started = Date()
        let runtime = try makeRuntime(invocation)
        let loaded = Date().timeIntervalSince(started)
        Timing.mark("runtime")
        let server = PlamoServer(
            // Repeats are the common case: the mods re-translate a redrawn or scrolled screen.
            router: Router(
                provider: CachingProvider(inner: runtime), defaultMaxTokens: invocation.maxTokens),
            configuration: .init(
                port: invocation.port,
                socketPath: invocation.socketPath,
                idleSeconds: invocation.idleSeconds,
                lukewarmSeconds: invocation.lukewarmSeconds),
            onLukewarm: { await runtime.releaseWeights() })
        // The listener comes before the warm-up: the mods poll /v1/models, and a request that
        // arrives mid-warm-up waits for the same kernels the warm-up is compiling anyway.
        let endpoints = try await server.start()
        Timing.mark("listening")
        print("ready on \(endpoints)")
        let warm = try await runtime.warmUp()
        Timing.mark("warm-up")
        print(loadReport(loaded: loaded, warm: warm))
        // The listeners own their queues; this suspends until the idle timer stops the server. A
        // bare withCheckedContinuation here was never resumed, so the process outlived its idle
        // timeout and kept 560 MB resident. (dispatchMain() would trap: async main is on the main
        // queue.)
        await server.waitUntilStopped()
    }
}

internal let invocation: Invocation
Timing.mark("process")
do {
    invocation = try Invocation(arguments: Array(CommandLine.arguments.dropFirst()))
} catch {
    report("\(error)")
    exit(1)
}
MLXMemory.configure()
Timing.mark("mlx")
do {
    try await run(invocation)
} catch {
    report("\(error)")
    exit(1)
}
