import Foundation
import Testing
@testable import PlamoCore

@Test func parsesServeCommand() throws {
    let invocation = try Invocation(arguments: [
        "serve", "--model", "/models/plamo", "--port", "1234", "--idle-seconds", "30",
        "--max-tokens", "8", "hello", "world",
    ])
    #expect(invocation.command == .serve)
    #expect(invocation.modelDirectory.path == "/models/plamo")
    #expect(invocation.tokenizerBlob.path == "/models/plamo/tokenizer.bin")
    #expect(invocation.port == 1234)
    #expect(invocation.idleSeconds == 30)
    #expect(invocation.maxTokens == 8)
    #expect(invocation.prompt == "hello world")
}

@Test func parsesDefaults() throws {
    let invocation = try Invocation(arguments: ["load", "--model", "/m"])
    #expect(invocation.command == .load)
    #expect(invocation.port == Invocation.defaultPort)
    #expect(invocation.idleSeconds == Invocation.defaultIdleSeconds)
    #expect(invocation.maxTokens == Invocation.defaultMaxTokens)
    #expect(invocation.prompt.isEmpty)
}

@Test func acceptsTokenizerOverride() throws {
    let invocation = try Invocation(arguments: [
        "tokenize", "--model", "/m", "--tokenizer", "/blobs/t.bin",
    ])
    #expect(invocation.tokenizerBlob.path == "/blobs/t.bin")
}

@Test func ignoresValueLessFlags() throws {
    let invocation = try Invocation(arguments: [
        "serve", "--model", "/m", "--trust-remote-code",
    ])
    #expect(invocation.command == .serve)
    #expect(invocation.modelDirectory.path == "/m")
}

@Test func rejectsMissingCommand() {
    #expect(throws: PlamoError.missingCommand) {
        try Invocation(arguments: ["--model", "/m"])
    }
}

@Test func rejectsUnknownCommand() {
    #expect(throws: PlamoError.unknownCommand("translate")) {
        try Invocation(arguments: ["translate", "--model", "/m"])
    }
}

@Test func rejectsMissingModel() {
    #expect(throws: PlamoError.missingOption(name: "model")) {
        try Invocation(arguments: ["serve"])
    }
}

@Test(arguments: [
    ["serve", "--model", "/m", "--port", "70000"],
    ["serve", "--model", "/m", "--port", "http"],
])
func rejectsBadPort(_ arguments: [String]) {
    #expect(throws: PlamoError.self) { try Invocation(arguments: arguments) }
}

@Test(arguments: [
    ["serve", "--model", "/m", "--idle-seconds", "0"],
    ["serve", "--model", "/m", "--idle-seconds", "soon"],
    ["serve", "--model", "/m", "--max-tokens", "0"],
    ["serve", "--model", "/m", "--max-tokens", "many"],
])
func rejectsBadNumbers(_ arguments: [String]) {
    #expect(throws: PlamoError.self) { try Invocation(arguments: arguments) }
}

@Test func commandNamesMatchTheBinary() {
    #expect(Command.allCases.map(\.rawValue) == ["tokenize", "load", "generate", "serve", "debug"])
}
