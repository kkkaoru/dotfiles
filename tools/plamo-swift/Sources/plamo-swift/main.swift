import Darwin
import Foundation
import MLX

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data("plamo-swift: \(message)\n".utf8))
    exit(1)
}

var options = [String: String]()
var positional = [String]()
var arguments = CommandLine.arguments.dropFirst()
if let command = arguments.first {
    arguments = arguments.dropFirst()
    options["command"] = command
}
while let argument = arguments.first {
    arguments = arguments.dropFirst()
    if argument.hasPrefix("--"), let next = arguments.first, !next.hasPrefix("--") {
        options[String(argument.dropFirst(2))] = next
        arguments = arguments.dropFirst()
    } else if argument.hasPrefix("--") {
        options[String(argument.dropFirst(2))] = "true"
    } else {
        positional.append(argument)
    }
}

@MainActor
func required(_ key: String) -> String {
    guard let value = options[key] else { fail("missing --\(key)") }
    return value
}

let modelDirectory = URL(fileURLWithPath: required("model"))
let tokenizerBlob = URL(fileURLWithPath: required("tokenizer"))
let tokenizer = try PlamoTokenizer(
    blob: tokenizerBlob, meta: tokenizerBlob.deletingPathExtension().appendingPathExtension("json"))
switch options["command"] {
case "tokenize":
    print(tokenizer.encode(positional.joined(separator: " ")).map(String.init).joined(separator: " "))
    exit(0)
case "load":
    let started = Date()
    let model = try PlamoModel.load(directory: modelDirectory)
    MLX.eval(model.logits(ids: [model.config.bosTokenId]))
    print(String(format: "ready in %.3f s", Date().timeIntervalSince(started)))
    exit(0)
case "debug", "generate", "serve":
    break
default:
    fail("usage: plamo-swift <tokenize|load|generate|serve> --model <dir> --tokenizer <blob>")
}

let loadStarted = Date()
let model = try PlamoModel.load(directory: modelDirectory)
MLX.eval(model.logits(ids: [model.config.bosTokenId]))
let loadSeconds = Date().timeIntervalSince(loadStarted)

if options["command"] == "debug" {
    let ids = promptIDs(positional.joined(separator: " "))
    var captured = [(String, MLXArray)]()
    let scores = model.logits(ids: ids) { stage, hidden in captured.append((stage, hidden))
    }.asArray(Float.self)
    for (stage, hidden) in captured {
        let last = hidden[0..., (hidden.dim(1) - 1)..<hidden.dim(1), 0...]
        let values = last.asArray(Float.self)
        let norm = sqrt(values.reduce(0) { $0 + $1 * $1 })
        print(
            "\(stage) norm \(String(format: "%.4f", norm)) "
                + values.prefix(3).map { String(format: "%.4f", $0) }.joined(separator: " "))
    }
    let top = scores.enumerated().sorted { $0.element > $1.element }.prefix(5)
    print("prompt ids: \(ids)")
    print("eos: \(model.config.eosTokenId)")
    print(
        "top: "
            + top.map { "\($0.offset):\(String(format: "%.3f", $0.element))" }.joined(
                separator: " "))
    exit(0)
}

@MainActor
func promptIDs(_ text: String) -> [Int32] {
    var ids = tokenizer.encode(text)
    // The Python tokenizer prepends BOS; the model was trained with it.
    if ids.first != model.config.bosTokenId { ids.insert(model.config.bosTokenId, at: 0) }
    return ids
}

@MainActor
func complete(prompt: String, maxTokens: Int, stop: [String]) -> (String, Int, Double) {
    let started = Date()
    let produced = model.generate(
        prompt: promptIDs(prompt), maxTokens: maxTokens, stopIDs: Set(model.config.eosTokenId))
    var text = tokenizer.decode(produced)
    for marker in stop {
        if let range = text.range(of: marker) { text = String(text[..<range.lowerBound]) }
    }
    return (text, produced.count, Date().timeIntervalSince(started))
}

if options["command"] == "generate" {
    let prompt = positional.joined(separator: " ")
    let maxTokens = Int(options["max-tokens"] ?? "64")!
    let started = Date()
    let produced = model.generate(
        prompt: promptIDs(prompt), maxTokens: maxTokens, stopIDs: Set(model.config.eosTokenId))
    print("ids " + produced.map(String.init).joined(separator: " "))
    print(
        String(
            format: "load %.3f s, %d tokens in %.3f s", loadSeconds, produced.count,
            Date().timeIntervalSince(started)))
    print(tokenizer.decode(produced))
    exit(0)
}

let port = UInt16(options["port"] ?? "18555")!
let idleSeconds = Double(options["idle-seconds"] ?? "600")!
print(String(format: "loaded in %.3f s", loadSeconds))

final class Activity: @unchecked Sendable {
    var last = Date()
    var busy = false
}
let activity = Activity()

signal(SIGPIPE, SIG_IGN)
let listener = socket(AF_INET, SOCK_STREAM, 0)
guard listener >= 0 else { fail("socket: \(String(cString: strerror(errno)))") }
var reuse: Int32 = 1
setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))
var address = sockaddr_in()
address.sin_family = sa_family_t(AF_INET)
address.sin_port = port.bigEndian
address.sin_addr.s_addr = inet_addr("127.0.0.1")
let bound = withUnsafePointer(to: &address) { pointer in
    pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(listener, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
    }
}
guard bound == 0, listen(listener, 128) == 0 else {
    fail("bind 127.0.0.1:\(port): \(String(cString: strerror(errno)))")
}
print("ready on 127.0.0.1:\(port)")

Thread.detachNewThread {
    while true {
        Thread.sleep(forTimeInterval: 5)
        if !activity.busy, Date().timeIntervalSince(activity.last) > idleSeconds {
            exit(0)
        }
    }
}

func respond(_ client: Int32, status: String, body: String) {
    let payload = Data(body.utf8)
    let head = "HTTP/1.1 \(status)\r\nContent-Type: application/json\r\nContent-Length: \(payload.count)\r\nConnection: close\r\n\r\n"
    var out = Data(head.utf8)
    out.append(payload)
    out.withUnsafeBytes { raw in
        var sent = 0
        while sent < raw.count {
            let written = write(client, raw.baseAddress!.advanced(by: sent), raw.count - sent)
            if written <= 0 { return }
            sent += written
        }
    }
}

func readRequest(_ client: Int32) -> (method: String, path: String, body: Data)? {
    var buffer = Data()
    var chunk = [UInt8](repeating: 0, count: 65536)
    var headerEnd: Range<Data.Index>? = nil
    while headerEnd == nil {
        let count = read(client, &chunk, chunk.count)
        if count <= 0 { return nil }
        buffer.append(contentsOf: chunk[0..<count])
        headerEnd = buffer.range(of: Data("\r\n\r\n".utf8))
        if buffer.count > 1 << 22 { return nil }
    }
    let head = String(decoding: buffer[..<headerEnd!.lowerBound], as: UTF8.self)
    let lines = head.split(separator: "\r\n")
    guard let requestLine = lines.first else { return nil }
    let parts = requestLine.split(separator: " ")
    guard parts.count >= 2 else { return nil }
    var contentLength = 0
    for line in lines.dropFirst() {
        let pieces = line.split(separator: ":", maxSplits: 1)
        if pieces.count == 2, pieces[0].lowercased() == "content-length" {
            contentLength = Int(pieces[1].trimmingCharacters(in: .whitespaces)) ?? 0
        }
    }
    var body = Data(buffer[headerEnd!.upperBound...])
    while body.count < contentLength {
        let count = read(client, &chunk, chunk.count)
        if count <= 0 { break }
        body.append(contentsOf: chunk[0..<count])
    }
    return (String(parts[0]), String(parts[1]), body)
}

let modelsJSON = """
{"object":"list","data":[{"id":"plamo-3-translate","object":"model","created":0,"owned_by":"local"}]}
"""

while true {
    let client = accept(listener, nil, nil)
    guard client >= 0 else { continue }
    defer { close(client) }
    guard let request = readRequest(client) else { continue }
    activity.last = Date()
    let json = (try? JSONSerialization.jsonObject(with: request.body)) as? [String: Any] ?? [:]

    switch (request.method, request.path) {
    case ("GET", "/v1/models"):
        respond(client, status: "200 OK", body: modelsJSON)
    case ("POST", "/v1/completions"):
        activity.busy = true
        defer { activity.busy = false }
        let prompt = (json["prompt"] as? String) ?? ""
        let maxTokens = (json["max_tokens"] as? Int) ?? 64
        let stop = (json["stop"] as? [String]) ?? []
        let (text, count, _) = complete(prompt: prompt, maxTokens: maxTokens, stop: stop)
        let escaped = String(decoding: try! JSONSerialization.data(withJSONObject: [text]), as: UTF8.self)
        let textJSON = String(escaped.dropFirst().dropLast())
        respond(
            client, status: "200 OK",
            body: """
                {"id":"plamo-3-translate","object":"text_completion","created":0,"model":"plamo-3-translate",\
                "choices":[{"index":0,"text":\(textJSON),"finish_reason":"stop"}],\
                "usage":{"prompt_tokens":0,"completion_tokens":\(count),"total_tokens":\(count)}}
                """)
    case ("POST", "/v1/chat/completions"):
        activity.busy = true
        defer { activity.busy = false }
        let messages = (json["messages"] as? [[String: Any]]) ?? []
        let prompt = messages.compactMap { $0["content"] as? String }.joined(separator: "\n")
        let maxTokens = (json["max_tokens"] as? Int) ?? 64
        let (text, count, _) = complete(prompt: prompt, maxTokens: maxTokens, stop: [])
        let escaped = String(decoding: try! JSONSerialization.data(withJSONObject: [text]), as: UTF8.self)
        let textJSON = String(escaped.dropFirst().dropLast())
        respond(
            client, status: "200 OK",
            body: """
                {"id":"plamo-3-translate","object":"chat.completion","created":0,"model":"plamo-3-translate",\
                "choices":[{"index":0,"message":{"role":"assistant","content":\(textJSON)},"finish_reason":"stop"}],\
                "usage":{"prompt_tokens":0,"completion_tokens":\(count),"total_tokens":\(count)}}
                """)
    default:
        respond(client, status: "404 Not Found", body: #"{"error":"not found"}"#)
    }
}
