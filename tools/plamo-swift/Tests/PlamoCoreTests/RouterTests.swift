import Foundation
import Testing

@testable import PlamoCore

private let stub = StubProvider()

@Test func listsTheModel() async {
    let response = await Router(provider: stub).respond(
        to: HTTPRequest(method: "GET", path: "/v1/models", body: Data()))
    #expect(response.status == "200 OK")
    #expect(response.body.contains("\"plamo-3-translate\""))
}

@Test func answers404ForAnotherPath() async {
    let response = await Router(provider: stub).respond(
        to: HTTPRequest(method: "GET", path: "/v1/unknown", body: Data()))
    #expect(response.status == "404 Not Found")
    #expect(response.body.contains("not found"))
}

@Test func rejectsAnInvalidJSONBody() async {
    let router = Router(provider: stub)
    for path in ["/v1/completions", "/v1/chat/completions"] {
        let response = await router.respond(
            to: HTTPRequest(method: "POST", path: path, body: Data("{".utf8)))
        #expect(response.status == "400 Bad Request")
        #expect(response.body.contains("invalid JSON body"))
    }
}

@Test func rejectsANonPositiveTokenCap() async {
    let response = await Router(provider: stub).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions",
            body: Data(#"{"prompt":"hi","max_tokens":0}"#.utf8)))
    #expect(response.status == "400 Bad Request")
    #expect(response.body.contains("max_tokens 0"))
}

@Test func rejectsATokenCapAboveTheLimit() async {
    // `generate` reserves one entry per token before decoding on the actor that serves every
    // request, so an unbounded cap is 400 MB of reservation and hours of decode.
    let response = await Router(provider: stub).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions",
            body: Data(#"{"prompt":"hi","max_tokens":100000000}"#.utf8)))
    #expect(response.status == "400 Bad Request")
    #expect(response.body.contains("max_tokens 100000000"))
}

@Test func clampsADefaultTokenCapToTheLimit() async {
    let recorder = Recorder()
    let router = Router(provider: StubProvider(recorder: recorder), defaultMaxTokens: 1_000_000)
    _ = await router.respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions", body: Data(#"{"prompt":"hi"}"#.utf8)))
    #expect(await recorder.requests.last?.maxTokens == Router.maximumMaxTokens)
}

@Test func reportsAProviderFailureAsA500() async {
    var provider = StubProvider()
    provider.failure = PlamoError.invalidRequest(reason: "bad")
    let rejected = await Router(provider: provider).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions", body: Data(#"{"prompt":"hi"}"#.utf8)))
    #expect(rejected.status == "400 Bad Request")

    var broken = StubProvider()
    broken.failure = PlamoError.unsupportedModelLayout(reason: "weights")
    let failed = await Router(provider: broken).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions", body: Data(#"{"prompt":"hi"}"#.utf8)))
    #expect(failed.status == "500 Internal Server Error")
}

@Test func passesTheStopListAndTheDefaultTokenCapToTheProvider() async {
    let recorder = Recorder()
    let router = Router(
        provider: StubProvider(recorder: recorder), defaultMaxTokens: 12)
    _ = await router.respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions",
            body: Data(#"{"prompt":"hi","stop":["\n","x"]}"#.utf8)))
    let request = await recorder.requests.first
    #expect(request?.maxTokens == 12)
    #expect(request?.stop == ["\n", "x"])
}

@Test func acceptsASingleStopString() async {
    let recorder = Recorder()
    let router = Router(provider: StubProvider(recorder: recorder))
    _ = await router.respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions",
            body: Data(#"{"prompt":"hi","stop":"\n"}"#.utf8)))
    #expect(await recorder.requests.first?.stop == ["\n"])
}

@Test func joinsTheChatMessagesIntoOnePrompt() async {
    let recorder = Recorder()
    let router = Router(provider: StubProvider(recorder: recorder))
    let json =
        #"{"messages":[{"role":"user","content":"one"},{"role":"user","content":"two"},"#
        + #"{"role":"user"}]}"#
    let body = Data(json.utf8)
    let response = await router.respond(
        to: HTTPRequest(method: "POST", path: "/v1/chat/completions", body: body))
    #expect(response.status == "200 OK")
    #expect(await recorder.requests.first?.prompt == "one\ntwo")
    #expect(response.body.contains("chat.completion"))
}

@Test func rejectsANonPositiveTokenCapOnTheChatPath() async {
    let response = await Router(provider: stub).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/chat/completions",
            body: Data(#"{"messages":[{"role":"user","content":"hi"}],"max_tokens":0}"#.utf8)))
    #expect(response.status == "400 Bad Request")
    #expect(response.body.contains("max_tokens 0"))
}

@Test func reportsAChatProviderFailure() async {
    var provider = StubProvider()
    provider.failure = PlamoError.unsupportedModelLayout(reason: "weights")
    let response = await Router(provider: provider).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/chat/completions",
            body: Data(#"{"messages":[{"role":"user","content":"hi"}]}"#.utf8)))
    #expect(response.status == "500 Internal Server Error")
}

@Test func escapesQuotesInAnError() async {
    var provider = StubProvider()
    provider.failure = PlamoError.invalidRequest(reason: "a \"quoted\" reason")
    let response = await Router(provider: provider).respond(
        to: HTTPRequest(
            method: "POST", path: "/v1/completions", body: Data(#"{"prompt":"hi"}"#.utf8)))
    #expect(response.body.contains(#"a \"quoted\" reason"#))
}
