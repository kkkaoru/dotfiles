import Foundation

/// PLaMo 3's Unigram tokenizer, replayed over the trie the Python tokenizer builds.
///
/// The trie itself is dumped once by `scripts/dump-tokenizer.py` (960 791 rows, 107 521 tokens):
/// rebuilding it needs the whole vocabulary sorted by reversed suffix, which is the one part of
/// this tokenizer that is cheaper to compute once in Python than every time in Swift.
public final class PlamoTokenizer {
    private static let pieceLengthColumn = 0
    private static let tokenIDColumn = 1
    private static let scoreColumn = 2
    private static let pieceIDColumn = 3
    private static let columns = 4
    private static let byteTokenCount = 256

    /// A bounds-checked little-endian reader: a truncated or foreign blob throws instead of reading
    /// past the end of the file.
    private struct Reader {
        let bytes: [UInt8]
        private(set) var offset = 0

        mutating func int32() throws -> Int32 {
            let value = try integer(byteCount: 4)
            return Int32(bitPattern: UInt32(truncatingIfNeeded: value))
        }

        mutating func int64() throws -> Int64 {
            Int64(bitPattern: UInt64(truncatingIfNeeded: try integer(byteCount: 8)))
        }

        mutating func raw(_ count: Int) throws -> ArraySlice<UInt8> {
            guard count >= 0, offset + count <= bytes.count else {
                throw PlamoError.corruptTokenizerBlob(reason: "truncated at \(offset)")
            }
            defer { offset += count }
            return bytes[offset..<(offset + count)]
        }

        /// Little-endian, as `scripts/dump-tokenizer.py` writes it.
        private mutating func integer(byteCount: Int) throws -> UInt64 {
            var value: UInt64 = 0
            for (index, byte) in try raw(byteCount).enumerated() {
                value |= UInt64(byte) << (8 * index)
            }
            return value
        }
    }

    private let tokens: [String]
    private let table: [Int32]
    private let rowCount: Int
    private let suffixIDs: [Int64: Int32]
    private let byteTokens: [Int32]
    private let boundary: Int32
    private let boundaryTokenID: Int32
    private let unknownScore: Int32
    private let invalidScore: Int32
    private let specialPattern: NSRegularExpression
    private let breakPattern: NSRegularExpression

    /// Loads the dumped blob and its metadata; a malformed pair throws rather than half-loading.
    public init(blob: URL, meta: URL) throws {
        let decoded = try JSONDecoder().decode(
            PlamoTokenizerMeta.self, from: try Data(contentsOf: meta))
        guard let boundaryScalar = decoded.boundaryChar.unicodeScalars.first else {
            throw PlamoError.corruptTokenizerBlob(reason: "empty boundary character")
        }
        boundary = Int32(bitPattern: boundaryScalar.value)
        boundaryTokenID = decoded.boundaryTokenID
        unknownScore = decoded.unknownScore
        invalidScore = decoded.invalidScore
        specialPattern = try NSRegularExpression(pattern: decoded.specialPattern)
        breakPattern = try NSRegularExpression(pattern: decoded.breakPattern)

        var reader = Reader(bytes: [UInt8](try Data(contentsOf: blob)))
        guard try reader.raw(4).elementsEqual(Data("PLT1".utf8)) else {
            throw PlamoError.corruptTokenizerBlob(reason: "bad magic")
        }
        rowCount = Int(try reader.int32())
        guard rowCount >= 0 else { throw PlamoError.corruptTokenizerBlob(reason: "row count") }
        var table = [Int32]()
        table.reserveCapacity(rowCount * PlamoTokenizer.columns)
        for _ in 0..<(rowCount * PlamoTokenizer.columns) { table.append(try reader.int32()) }
        self.table = table

        let pairCount = Int(try reader.int32())
        guard pairCount >= 0 else { throw PlamoError.corruptTokenizerBlob(reason: "pair count") }
        var suffixIDs = [Int64: Int32](minimumCapacity: pairCount)
        for _ in 0..<pairCount { suffixIDs[try reader.int64()] = try reader.int32() }
        self.suffixIDs = suffixIDs

        var byteTokens = [Int32]()
        byteTokens.reserveCapacity(PlamoTokenizer.byteTokenCount)
        for _ in 0..<PlamoTokenizer.byteTokenCount { byteTokens.append(try reader.int32()) }
        self.byteTokens = byteTokens

        let tokenCount = Int(try reader.int32())
        guard tokenCount >= 0 else { throw PlamoError.corruptTokenizerBlob(reason: "token count") }
        var tokens = [String]()
        tokens.reserveCapacity(tokenCount)
        for _ in 0..<tokenCount {
            let length = Int(try reader.int32())
            let slice = try reader.raw(length)
            guard let token = String(bytes: slice, encoding: .utf8) else {
                throw PlamoError.corruptTokenizerBlob(reason: "token \(tokens.count) is not UTF-8")
            }
            tokens.append(token)
        }
        self.tokens = tokens
    }

    /// Same two boundary passes as the Python tokenizer: special tokens first, then runs of spaces
    /// and repeated characters, each wrapped in the boundary character so the Viterbi walk cannot
    /// shift a token boundary across them.
    public func encode(_ text: String) -> [Int32] {
        let marked = substitute(breakPattern, in: substitute(specialPattern, in: text))
        return viterbi(marked.unicodeScalars.map { Int32(bitPattern: $0.value) })
    }

    /// The prompt the model was trained on starts with BOS; adding it here keeps every caller from
    /// having to remember that.
    public func promptIDs(_ text: String, bos: Int32) -> [Int32] {
        let ids = encode(text)
        return ids.first == bos ? ids : [bos] + ids
    }

    /// Turns ids back into text, dropping the boundary and special tokens.
    public func decode(_ ids: [Int32]) throws -> String {
        var bytes = [UInt8]()
        for id in ids where id != boundaryTokenID {
            let index = Int(id)
            guard index >= 0, index < tokens.count else {
                throw PlamoError.invalidRequest(reason: "token id \(id)")
            }
            let token = tokens[index]
            if token.count == 6, token.hasPrefix("<0x"), token.hasSuffix(">") {
                guard let byte = UInt8(token.dropFirst(3).dropLast(), radix: 16) else {
                    throw PlamoError.corruptTokenizerBlob(reason: "byte token \(token)")
                }
                bytes.append(byte)
            } else {
                bytes.append(contentsOf: Array(token.utf8))
            }
        }
        if let text = String(bytes: bytes, encoding: .utf8) { return text }
        // A completion cut at `max_tokens` can end mid-character: the byte-fallback tokens then
        // carry the first one or two bytes of a three-byte character and nothing more. Python's
        // tokenizer decodes those with `errors="ignore"`, so dropping the truncated tail keeps the
        // partial translation instead of turning the whole request into a 400. Only a tail that is
        // an incomplete sequence goes: a byte that cannot start one is still corruption.
        if let tail = Self.truncatedTail(bytes),
            let text = String(bytes: bytes.dropLast(tail), encoding: .utf8)
        {
            return text
        }
        throw PlamoError.invalidRequest(reason: "token bytes are not UTF-8")
    }

    /// The byte count of an incomplete UTF-8 sequence at the end of `bytes`, or nil when the tail
    /// is not a truncation of one (a byte that cannot start a sequence, or one already complete).
    private static func truncatedTail(_ bytes: [UInt8]) -> Int? {
        var index = bytes.count - 1
        while index >= 0, bytes[index] & 0xC0 == 0x80 { index -= 1 }
        guard index >= 0 else { return nil }
        let expected: Int
        switch bytes[index] {
        case 0xC2...0xDF: expected = 2
        case 0xE0...0xEF: expected = 3
        case 0xF0...0xF4: expected = 4
        default: return nil
        }
        let present = bytes.count - index
        return present < expected ? present : nil
    }

    private func substitute(_ pattern: NSRegularExpression, in text: String) -> String {
        let boundaryText = String(boundaryScalar)
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        var result = ""
        var cursor = text.startIndex
        for match in pattern.matches(in: text, range: range) {
            guard let matchRange = Range(match.range, in: text) else { continue }
            result += text[cursor..<matchRange.lowerBound]
            result += boundaryText + text[matchRange] + boundaryText
            cursor = matchRange.upperBound
        }
        result += text[cursor...]
        return result
    }

    /// The boundary character as a scalar, rebuilt from the id the blob stores.
    private var boundaryScalar: Unicode.Scalar {
        Unicode.Scalar(UInt32(bitPattern: boundary)) ?? Unicode.Scalar(UInt8(0))
    }

    /// The Python `_encode` walk, line for line: scores run from the end, the longest piece wins a
    /// tie, and a piece with the unknown score falls back to byte tokens.
    private func viterbi(_ data: [Int32]) -> [Int32] {
        let count = data.count
        var scores = [Int64](repeating: 1 << 60, count: count + 1)
        scores[count] = 0
        var pieceLengths = [Int32](repeating: 0, count: count + 1)
        var tokenIDs = [Int32](repeating: -1, count: count + 1)
        var tokenCounts = [Int32](repeating: 0, count: count + 1)

        var suffixID: Int32 = 0
        var index = count - 1
        while index >= 0 {
            let scalar = data[index]
            var row = Int(suffixID)
            while row < rowCount {
                let key =
                    (Int64(scalar) << 32)
                    | Int64(table[row * PlamoTokenizer.columns + Self.pieceIDColumn])
                suffixID = suffixIDs[key] ?? 0
                let rowScore = table[row * PlamoTokenizer.columns + Self.scoreColumn]
                if suffixID > 0 || rowScore == unknownScore { break }
                row += 1
            }

            row = Int(suffixID)
            while row < rowCount {
                let score = table[row * PlamoTokenizer.columns + Self.scoreColumn]
                if score > invalidScore {
                    let pieceLength = table[row * PlamoTokenizer.columns + Self.pieceLengthColumn]
                    let candidate = scores[index + Int(pieceLength)] - Int64(score)
                    if candidate < scores[index] {
                        scores[index] = candidate
                        pieceLengths[index] = pieceLength
                        tokenIDs[index] = table[row * PlamoTokenizer.columns + Self.tokenIDColumn]
                        tokenCounts[index] = tokenCounts[index + Int(pieceLength)] + 1
                        if score == unknownScore {
                            tokenCounts[index] += Int32(PlamoTokenizer.byteCount(scalar) - 1)
                        }
                    }
                }
                if score == unknownScore { break }
                row += 1
            }
            index -= 1
        }

        return walk(
            data, pieceLengths: pieceLengths, tokenIDs: tokenIDs, tokenCount: tokenCounts[0])
    }

    /// Replays the winning pieces left to right; a position with no token falls back to bytes.
    private func walk(
        _ data: [Int32], pieceLengths: [Int32], tokenIDs: [Int32], tokenCount: Int32
    ) -> [Int32] {
        var ids = [Int32]()
        ids.reserveCapacity(Int(tokenCount))
        var position = 0
        while position < data.count {
            if tokenIDs[position] >= 0 {
                if tokenIDs[position] != boundaryTokenID { ids.append(tokenIDs[position]) }
            } else {
                let scalar = data[position]
                let byteCount = PlamoTokenizer.byteCount(scalar)
                for part in 0..<byteCount {
                    let lead: Int32 =
                        byteCount == 1 ? scalar : part == 0 ? (0xF00 >> byteCount) & 0xFF : 0x80
                    let shift = Int32((byteCount - part - 1) * 6)
                    ids.append(byteTokens[Int(lead | ((scalar >> shift) & 0x3F))])
                }
            }
            position += max(Int(pieceLengths[position]), 1)
        }
        return ids
    }

    /// UTF-8 length of one scalar: 1 to 4 bytes.
    private static func byteCount(_ scalar: Int32) -> Int {
        1 + (scalar >= 0x80 ? 1 : 0) + (scalar >= 0x800 ? 1 : 0) + (scalar >= 0x10000 ? 1 : 0)
    }
}

/// The tokenizer's `blob.json`: the scores and patterns the Python tokenizer carries in code.
private struct PlamoTokenizerMeta: Decodable {
    let boundaryChar: String
    let boundaryTokenID: Int32
    let unknownScore: Int32
    let invalidScore: Int32
    let specialPattern: String
    let breakPattern: String

    private enum CodingKeys: String, CodingKey {
        case boundaryChar = "boundary_char"
        case boundaryTokenID = "boundary_token_id"
        case unknownScore = "unknown_score"
        case invalidScore = "invalid_score"
        case specialPattern = "special_pattern"
        case breakPattern = "break_pattern"
    }
}
