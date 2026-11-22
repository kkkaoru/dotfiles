import Foundation

/// The checkpoint's `config.json`. Field names are the checkpoint's own, so a model that changes
/// shape fails at load with `invalidConfig` instead of producing silent garbage.
public struct PlamoConfig: Decodable, Sendable {
    public struct RopeParameters: Decodable, Sendable {
        public let ropeType: String
        public let ropeTheta: Float
        public let factor: Float?
        public let originalMaxPositionEmbeddings: Int?
        public let betaFast: Float?
        public let betaSlow: Float?

        enum CodingKeys: String, CodingKey {
            case ropeType = "rope_type"
            case ropeTheta = "rope_theta"
            case factor
            case originalMaxPositionEmbeddings = "original_max_position_embeddings"
            case betaFast = "beta_fast"
            case betaSlow = "beta_slow"
        }
    }

    public let hiddenSize: Int
    public let numHiddenLayers: Int
    public let numAttentionHeads: Int
    public let numKeyValueHeads: Int
    public let headDim: Int
    public let intermediateSize: Int
    public let vocabSize: Int
    public let rmsNormEps: Float
    public let windowSize: Int
    public let slidingWindowPattern: Int
    public let ropeParameters: [String: RopeParameters]
    public let eosTokenId: [Int32]
    public let bosTokenId: Int32

    enum CodingKeys: String, CodingKey {
        case hiddenSize = "hidden_size"
        case numHiddenLayers = "num_hidden_layers"
        case numAttentionHeads = "num_attention_heads"
        case numKeyValueHeads = "num_key_value_heads"
        case headDim = "head_dim"
        case intermediateSize = "intermediate_size"
        case vocabSize = "vocab_size"
        case rmsNormEps = "rms_norm_eps"
        case windowSize = "window_size"
        case slidingWindowPattern = "sliding_window_pattern"
        case ropeParameters = "rope_parameters"
        case eosTokenId = "eos_token_id"
        case bosTokenId = "bos_token_id"
    }

    /// The layout the attention and MLP code assumes. Checked once at load: a mismatch here is a
    /// wrong checkpoint, not a wrong request.
    public func validate() throws {
        // PLaMo 3's q/k/v projections widen past hidden_size (1536 -> 16 x 128), so the two are
        // only required to be positive; the projections carry their own shapes.
        guard hiddenSize > 0 else { throw PlamoError.invalidConfig(field: "hidden_size") }
        guard headDim > 0 else { throw PlamoError.invalidConfig(field: "head_dim") }
        guard numAttentionHeads > 0 else {
            throw PlamoError.invalidConfig(field: "num_attention_heads")
        }
        guard numHiddenLayers > 0 else { throw PlamoError.invalidConfig(field: "num_hidden_layers") }
        guard numKeyValueHeads > 0, numAttentionHeads % numKeyValueHeads == 0 else {
            throw PlamoError.invalidConfig(field: "num_key_value_heads")
        }
        guard intermediateSize > 0 else { throw PlamoError.invalidConfig(field: "intermediate_size") }
        guard vocabSize > 0 else { throw PlamoError.invalidConfig(field: "vocab_size") }
        guard windowSize > 0 else { throw PlamoError.invalidConfig(field: "window_size") }
        guard slidingWindowPattern > 0 else {
            throw PlamoError.invalidConfig(field: "sliding_window_pattern")
        }
        guard ropeParameters["full_attention"] != nil else {
            throw PlamoError.invalidConfig(field: "rope_parameters.full_attention")
        }
        guard ropeParameters["sliding_attention"] != nil else {
            throw PlamoError.invalidConfig(field: "rope_parameters.sliding_attention")
        }
    }

    /// Loads and validates `config.json`; a missing file is reported as such, not as a decode error.
    public static func load(directory: URL) throws -> PlamoConfig {
        let url = directory.appendingPathComponent("config.json")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw PlamoError.missingModelFile("config.json")
        }
        let config = try JSONDecoder().decode(PlamoConfig.self, from: try Data(contentsOf: url))
        try config.validate()
        return config
    }
}
