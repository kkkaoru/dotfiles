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

/// Counts the tokens a text prefills, BOS included. A client that has to fit a document into the
/// window asks this instead of estimating from characters: Japanese is about one token per
/// character, so a 4-tokens-per-character estimate halves the usable chunk.
public protocol TokenCounting: Sendable {
    func countTokens(_ text: String) async -> Int
}

/// Cuts a completion at its earliest stop marker. The order of the list must not decide the cut:
/// a client sends `["<|plamo:tag|>", "<|plamo:bos|>"]` meaning whichever appears first in the text,
/// and the old loop broke at the first marker that matched anywhere, keeping text after an earlier
/// one that was listed later.
public enum StopMarker {
    /// The text before the earliest marker, or the whole text when no marker appears.
    public static func truncate(_ text: String, at markers: [String]) -> String {
        var end = text.endIndex
        for marker in markers where !marker.isEmpty {
            if let range = text.range(of: marker), range.lowerBound < end {
                end = range.lowerBound
            }
        }
        return String(text[..<end])
    }
}
