import Foundation

/// One finished translation: the text, how many tokens produced it, and how long the decode took.
public struct Completion: Sendable, Equatable {
    public let text: String
    /// Tokens the caller sent: reported as `usage.prompt_tokens`, which clients bill from.
    public let promptTokens: Int
    public let tokens: Int
    public let seconds: Double

    public init(text: String, promptTokens: Int = 0, tokens: Int, seconds: Double) {
        self.text = text
        self.promptTokens = promptTokens
        self.tokens = tokens
        self.seconds = seconds
    }
}

/// A decoded request body: the prompt to translate and the limits the caller set.
public struct CompletionRequest: Sendable, Equatable {
    public let prompt: String
    public let maxTokens: Int
    public let stop: [String]

    public init(prompt: String, maxTokens: Int, stop: [String]) {
        self.prompt = prompt
        self.maxTokens = maxTokens
        self.stop = stop
    }
}

/// What the router needs from the model. The tests replace it with a stub, so every HTTP path is
/// exercised without loading 0.55 GB of weights.
public protocol CompletionProviding: Sendable {
    func complete(_ request: CompletionRequest) async throws -> Completion
}
