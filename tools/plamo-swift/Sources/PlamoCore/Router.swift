import Foundation

/// The OpenAI-compatible surface the mods speak: `GET /v1/models`, `POST /v1/completions` and
/// `POST /v1/chat/completions`. Pure request-to-response mapping, so every status code is a test.
public struct Router: Sendable {
    public static let modelID = "plamo-3-translate"
    /// The largest completion a request may ask for. `generate` reserves one entry per token before
    /// decoding and the decode runs on the one actor that serves every request, so a body with
    /// `max_tokens: 100000000` would both allocate 400 MB up front and hold the server for hours.
    /// 4096 is past any answer this model produces inside its 2048-token window.
    public static let maximumMaxTokens = 4_096
    /// The model's sliding window: 14 of its 16 layers see only this many keys, so a prompt past it
    /// is read by the two full-attention layers alone. Clients split longer text.
    public static let defaultWindowSize = 2_048
    public static let modelsJSON =
        #"{"object":"list","data":[{"id":"plamo-3-translate","object":"model","created":0,"#
        + #""owned_by":"local"}]}"#

    private let provider: any CompletionProviding
    private let counting: (any TokenCounting)?
    private let defaultMaxTokens: Int
    private let windowSize: Int

    public init(
        provider: any CompletionProviding,
        counting: (any TokenCounting)? = nil,
        defaultMaxTokens: Int = 64,
        windowSize: Int = Router.defaultWindowSize
    ) {
        self.provider = provider
        self.counting = counting
        self.defaultMaxTokens = defaultMaxTokens
        self.windowSize = windowSize
    }

    public func respond(to request: HTTPRequest) async -> HTTPResponse {
        switch (request.method, request.path) {
        case ("GET", "/v1/models"):
            return HTTPResponse(status: "200 OK", body: Router.modelsJSON)
        case ("POST", "/v1/completions"):
            return await completion(request)
        case ("POST", "/v1/chat/completions"):
            return await chat(request)
        case ("POST", "/v1/tokenize"):
            return await tokenize(request)
        default:
            return Router.error(status: "404 Not Found", message: "not found")
        }
    }

    private func completion(_ request: HTTPRequest) async -> HTTPResponse {
        let body: CompletionBody
        do {
            body = try JSONDecoder().decode(CompletionBody.self, from: request.body)
        } catch {
            return Router.error(status: "400 Bad Request", message: "invalid JSON body")
        }
        let stop = body.stop?.values ?? []
        let completion: Completion
        do {
            completion = try await provider.complete(
                CompletionRequest(
                    prompt: body.prompt ?? "",
                    maxTokens: try Router.maxTokens(body.maxTokens, default: defaultMaxTokens),
                    stop: stop))
        } catch {
            return Router.failure(error)
        }
        let payload = CompletionPayload(
            id: Router.modelID, object: "text_completion", model: Router.modelID,
            choices: [.init(index: 0, text: completion.text, finishReason: "stop")],
            usage: .init(prompt: completion.promptTokens, completion: completion.tokens))
        return Router.encode(payload)
    }

    /// Exact prompt-token counts, so a client can split a document at sentence boundaries before
    /// a request overflows the window. The counts are what `complete` would prefill for that text.
    private func tokenize(_ request: HTTPRequest) async -> HTTPResponse {
        guard let counting else {
            return Router.error(status: "501 Not Implemented", message: "tokenizer unavailable")
        }
        let body: TokenizeBody
        do {
            body = try JSONDecoder().decode(TokenizeBody.self, from: request.body)
        } catch {
            return Router.error(status: "400 Bad Request", message: "invalid JSON body")
        }
        var tokens = [Int]()
        tokens.reserveCapacity(body.texts.count)
        for text in body.texts {
            tokens.append(await counting.countTokens(text))
        }
        return Router.encode(
            TokenizePayload(tokens: tokens, window: windowSize, maxTokens: Router.maximumMaxTokens))
    }

    private func chat(_ request: HTTPRequest) async -> HTTPResponse {
        let body: ChatBody
        do {
            body = try JSONDecoder().decode(ChatBody.self, from: request.body)
        } catch {
            return Router.error(status: "400 Bad Request", message: "invalid JSON body")
        }
        let prompt = body.messages.compactMap(\.content).joined(separator: "\n")
        let completion: Completion
        do {
            completion = try await provider.complete(
                CompletionRequest(
                    prompt: prompt,
                    maxTokens: try Router.maxTokens(body.maxTokens, default: defaultMaxTokens),
                    stop: []))
        } catch {
            return Router.failure(error)
        }
        let choice = ChatChoice(
            index: 0,
            message: ChatMessage(role: "assistant", content: completion.text),
            finishReason: "stop")
        let payload = ChatPayload(
            id: Router.modelID, object: "chat.completion", model: Router.modelID,
            choices: [choice],
            usage: .init(prompt: completion.promptTokens, completion: completion.tokens))
        return Router.encode(payload)
    }

    private static func maxTokens(_ requested: Int?, default fallback: Int) throws -> Int {
        guard let requested else { return min(fallback, Router.maximumMaxTokens) }
        guard requested > 0, requested <= Router.maximumMaxTokens else {
            throw PlamoError.invalidRequest(reason: "max_tokens \(requested)")
        }
        return requested
    }

    private static func encode(_ payload: some Encodable) -> HTTPResponse {
        guard let data = try? JSONEncoder().encode(payload),
            let body = String(bytes: data, encoding: .utf8)
        else {
            return Router.error(status: "500 Internal Server Error", message: "encode failed")
        }
        return HTTPResponse(status: "200 OK", body: body)
    }

    private static func failure(_ error: any Error) -> HTTPResponse {
        switch error {
        case PlamoError.invalidRequest(let reason):
            return Router.error(status: "400 Bad Request", message: reason)
        default:
            return Router.error(status: "500 Internal Server Error", message: "\(error)")
        }
    }

    private static func error(status: String, message: String) -> HTTPResponse {
        let escaped = message.replacingOccurrences(of: "\"", with: "\\\"")
        return HTTPResponse(
            status: status,
            body: #"{"error":{"message":"\#(escaped)","type":"invalid_request_error"}}"#)
    }
}

// MARK: - Bodies

private struct TokenizeBody: Decodable {
    let texts: [String]
}

private struct TokenizePayload: Encodable {
    let tokens: [Int]
    let window: Int
    let maxTokens: Int

    private enum CodingKeys: String, CodingKey {
        case tokens
        case window
        case maxTokens = "max_tokens"
    }
}

private struct CompletionBody: Decodable {
    let prompt: String?
    let maxTokens: Int?
    let stop: StopSequence?

    private enum CodingKeys: String, CodingKey {
        case prompt
        case maxTokens = "max_tokens"
        case stop
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        prompt = try container.decodeIfPresent(String.self, forKey: .prompt)
        maxTokens = try container.decodeIfPresent(Int.self, forKey: .maxTokens)
        stop = try container.decodeIfPresent(StopSequence.self, forKey: .stop)
    }
}

/// `stop` is either one string or a list of them, the way OpenAI accepts it.
private enum StopSequence: Decodable {
    case one(String)
    case many([String])

    init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let many = try? container.decode([String].self) {
            self = .many(many)
        } else {
            self = .one(try container.decode(String.self))
        }
    }

    var values: [String] {
        switch self {
        case .one(let value): return [value]
        case .many(let values): return values
        }
    }
}

private struct ChatBody: Decodable {
    let messages: [ChatMessageBody]
    let maxTokens: Int?

    private enum CodingKeys: String, CodingKey {
        case messages
        case maxTokens = "max_tokens"
    }
}

private struct ChatMessageBody: Decodable {
    let content: String?
}

private struct Usage: Encodable {
    let promptTokens: Int
    let completionTokens: Int
    let totalTokens: Int

    private enum CodingKeys: String, CodingKey {
        case promptTokens = "prompt_tokens"
        case completionTokens = "completion_tokens"
        case totalTokens = "total_tokens"
    }

    init(prompt: Int, completion: Int) {
        promptTokens = prompt
        completionTokens = completion
        totalTokens = prompt + completion
    }
}

private struct CompletionChoice: Encodable {
    let index: Int
    let text: String
    let finishReason: String

    private enum CodingKeys: String, CodingKey {
        case index
        case text
        case finishReason = "finish_reason"
    }
}

private struct CompletionPayload: Encodable {
    let id: String
    let object: String
    let created = 0
    let model: String
    let choices: [CompletionChoice]
    let usage: Usage
}

private struct ChatMessage: Encodable {
    let role: String
    let content: String
}

private struct ChatChoice: Encodable {
    let index: Int
    let message: ChatMessage
    let finishReason: String

    private enum CodingKeys: String, CodingKey {
        case index
        case message
        case finishReason = "finish_reason"
    }
}

private struct ChatPayload: Encodable {
    let id: String
    let object: String
    let created = 0
    let model: String
    let choices: [ChatChoice]
    let usage: Usage
}
