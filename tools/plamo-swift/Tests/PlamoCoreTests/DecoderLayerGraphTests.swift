import Foundation
import Testing

@testable import PlamoCore

@Suite("Decoder layer graph")
struct DecoderLayerGraphTests {
    static let configJSON = """
        {
          "hidden_size": 1536,
          "num_hidden_layers": 16,
          "num_attention_heads": 16,
          "num_key_value_heads": 8,
          "head_dim": 128,
          "intermediate_size": 4096,
          "vocab_size": 100000,
          "rms_norm_eps": 1e-6,
          "window_size": 512,
          "sliding_window_pattern": 4,
          "rope_parameters": {
            "full_attention": { "rope_type": "default", "rope_theta": 1000000 },
            "sliding_attention": { "rope_type": "default", "rope_theta": 10000 }
          },
          "eos_token_id": [2],
          "bos_token_id": 1
        }
        """

    static func config() throws -> PlamoConfig {
        try JSONDecoder().decode(PlamoConfig.self, from: Data(configJSON.utf8))
    }

    @Test("a sliding layer's graph has the cache, the trim and the two norms")
    func slidingLayer() throws {
        let graph = DecoderLayerGraph(
            config: try Self.config(), weights: .init(prefix: "model", layer: 3), cacheLength: 512)
        let text = graph.render()

        #expect(text.contains("name: \"k_cache\""))
        #expect(text.contains("name: \"v_cache\""))
        #expect(text.contains("type: \"slice_by_size\""))
        #expect(text.contains("type: \"softmax\""))
        #expect(text.contains("type: \"rsqrt\""))
        #expect(text.contains("type: \"sigmoid\""))
        #expect(text.contains("model.layers.layers.3.mixer.qkv_proj.weight"))
        #expect(text.contains("model.layers.layers.3.mlp.gate_up_proj.weight"))
        // The layer must feed back the trimmed caches, not the untrimmed concatenation.
        #expect(text.contains("outputs: \"y\""))
        #expect(text.contains("outputs: \"keys_trimmed\""))
        #expect(text.contains("outputs: \"values_trimmed\""))
    }

    @Test("every layer of the checkpoint renders, and each names its own weights")
    func allLayers() throws {
        let config = try Self.config()
        for layer in 0..<config.numHiddenLayers {
            let text = DecoderLayerGraph(
                config: config, weights: .init(prefix: "model", layer: layer), cacheLength: 512
            ).render()
            #expect(text.contains("model.layers.layers.\(layer).mixer.o_proj.weight"))
            #expect(!text.contains("model.layers.layers.\(layer + 1).mixer.o_proj.weight"))
        }
    }
}
