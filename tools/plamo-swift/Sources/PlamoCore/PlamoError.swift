/// Every recoverable failure the server can report. Each case names the input that was wrong, so a
/// caller never has to parse a message to tell a bad flag from a corrupt file.
public enum PlamoError: Error, Equatable {
    case missingCommand
    case unknownCommand(String)
    case missingOption(name: String)
    case invalidOption(name: String, value: String)
    case invalidConfig(field: String)
    case corruptTokenizerBlob(reason: String)
    case missingModelFile(String)
    case unsupportedModelLayout(reason: String)
    case invalidRequest(reason: String)
    case listener(port: UInt16, reason: String)
    case connectionClosed
}

extension PlamoError: CustomStringConvertible {
    /// One line a user can act on: it names the flag, file or field that was wrong.
    public var description: String {
        switch self {
        case .missingCommand:
            return "usage: plamo-swift <tokenize|load|generate|serve|debug> --model <dir>"
        case .unknownCommand(let name):
            return "unknown command \(name)"
        case .missingOption(let name):
            return "missing --\(name)"
        case .invalidOption(let name, let value):
            return "invalid --\(name) \(value)"
        case .invalidConfig(let field):
            return "invalid config field \(field)"
        case .corruptTokenizerBlob(let reason):
            return "corrupt tokenizer blob: \(reason)"
        case .missingModelFile(let name):
            return "missing model file \(name)"
        case .unsupportedModelLayout(let reason):
            return "unsupported model layout: \(reason)"
        case .invalidRequest(let reason):
            return "invalid request: \(reason)"
        case .listener(let port, let reason):
            return "cannot listen on 127.0.0.1:\(port): \(reason)"
        case .connectionClosed:
            return "connection closed"
        }
    }
}
