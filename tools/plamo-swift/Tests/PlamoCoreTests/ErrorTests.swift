import Testing
@testable import PlamoCore

@Test func describesEveryError() {
    #expect(PlamoError.missingCommand.description.contains("usage"))
    #expect(PlamoError.unknownCommand("x").description == "unknown command x")
    #expect(PlamoError.missingOption(name: "model").description == "missing --model")
    #expect(PlamoError.invalidOption(name: "port", value: "x").description == "invalid --port x")
    #expect(PlamoError.invalidConfig(field: "head_dim").description == "invalid config field head_dim")
    #expect(PlamoError.corruptTokenizerBlob(reason: "magic").description == "corrupt tokenizer blob: magic")
    #expect(PlamoError.missingModelFile("a").description == "missing model file a")
    #expect(PlamoError.unsupportedModelLayout(reason: "x").description == "unsupported model layout: x")
    #expect(PlamoError.invalidRequest(reason: "y").description == "invalid request: y")
    #expect(PlamoError.listener(port: 1, reason: "busy").description.contains("127.0.0.1:1"))
    #expect(PlamoError.connectionClosed.description == "connection closed")
}
