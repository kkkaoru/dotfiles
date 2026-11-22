import Foundation

/// The OpenAI-compatible surface the mods speak: `GET /v1/models`, `POST /v1/completions` and
/// `POST /v1/chat/completions`. Pure request-to-response mapping, so every status code is a test.
public struct Router: Sendable {
    public static let modelID = "plamo-3-translate"
    public static let modelsJSON =
        #"{"object":"list","data":[{"id":"plamo-3-translate","object":"model","created":0,"#
        + #""owned_by":"local"}]}"#

    private let provider: any CompletionProviding
    private let defaultMaxTokens: Int

    public init(provider: any CompletionProviding, defaultMaxTokens: Int = 64) {
        self.provider = provider
        self.defaultMaxTokens = defaultMaxTokens
    }

    public func respond(to request: HTTPRequest) async -> HTTPResponse {
        switch (request.method, request.path) {
        case ("GET", "/v1/models"):
            return HTTPResponse(status: "200 OK", body: Router.modelsJSON)
        case ("POST", "/v1/completions"):
            return await completion(request)
        case ("POST", "/v1/chat/completions"):
            return await chat(request)
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
        guard let requested else { return fallback }
        guard requested > 0 else {
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
