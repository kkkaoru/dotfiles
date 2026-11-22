import Foundation

/// PLaMo 3's Unigram tokenizer, replayed over the trie the Python tokenizer builds.
///
/// The trie itself is dumped once by `scripts/dump-tokenizer.py` (960 791 rows, 107 521 tokens):
/// rebuilding it needs the whole vocabulary sorted by reversed suffix, which is the one part of
/// this tokenizer that is cheaper to compute once in Python than every time in Swift.
final class PlamoTokenizer {
    private static let pieceLengthColumn = 0
    private static let tokenIDColumn = 1
    private static let scoreColumn = 2
    private static let pieceIDColumn = 3

    private struct Meta: Decodable {
        let boundary_char: String
        let boundary_token_id: Int32
        let unknown_score: Int32
        let invalid_score: Int32
        let special_pattern: String
        let break_pattern: String
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

    init(blob: URL, meta: URL) throws {
        let metaData = try Data(contentsOf: meta)
        let decoded = try JSONDecoder().decode(Meta.self, from: metaData)
        boundary = Int32(decoded.boundary_char.unicodeScalars.first!.value)
        boundaryTokenID = decoded.boundary_token_id
        unknownScore = decoded.unknown_score
        invalidScore = decoded.invalid_score
        specialPattern = try NSRegularExpression(pattern: decoded.special_pattern)
        breakPattern = try NSRegularExpression(pattern: decoded.break_pattern)

        let raw = try Data(contentsOf: blob)
        var offset = 0
        func int32() -> Int32 {
            let value = raw.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: Int32.self) }
            offset += 4
            return Int32(littleEndian: value)
        }
        func int64() -> Int64 {
            let value = raw.withUnsafeBytes { $0.loadUnaligned(fromByteOffset: offset, as: Int64.self) }
            offset += 8
            return Int64(littleEndian: value)
        }

        guard raw.prefix(4) == Data("PLT1".utf8) else { throw CocoaError(.fileReadCorruptFile) }
        offset = 4
        rowCount = Int(int32())
        var table = [Int32]()
        table.reserveCapacity(rowCount * 4)
        for _ in 0..<(rowCount * 4) { table.append(int32()) }
        self.table = table

        let pairCount = Int(int32())
        var suffixIDs = [Int64: Int32](minimumCapacity: pairCount)
        for _ in 0..<pairCount {
            let key = int64()
            suffixIDs[key] = int32()
        }
        self.suffixIDs = suffixIDs

        var byteTokens = [Int32]()
        byteTokens.reserveCapacity(256)
        for _ in 0..<256 { byteTokens.append(int32()) }
        self.byteTokens = byteTokens

        let tokenCount = Int(int32())
        var tokens = [String]()
        tokens.reserveCapacity(tokenCount)
        for _ in 0..<tokenCount {
            let length = Int(int32())
            let slice = raw.subdata(in: offset..<(offset + length))
            tokens.append(String(decoding: slice, as: UTF8.self))
            offset += length
        }
        self.tokens = tokens
    }

    /// Same two boundary passes as the Python tokenizer: special tokens first, then runs of spaces
    /// and repeated characters, each wrapped in the boundary character so the Viterbi walk cannot
    /// shift a token boundary across them.
    func encode(_ text: String) -> [Int32] {
        let marked = substitute(breakPattern, in: substitute(specialPattern, in: text))
        let scalars = marked.unicodeScalars.map { Int32($0.value) }
        return viterbi(scalars)
    }

    func decode(_ ids: [Int32]) -> String {
        var bytes = [UInt8]()
        for id in ids where id != boundaryTokenID {
            let token = tokens[Int(id)]
            if token.count == 6, token.hasPrefix("<0x"), token.hasSuffix(">") {
                bytes.append(UInt8(token.dropFirst(3).dropLast(), radix: 16) ?? 0)
            } else {
                bytes.append(contentsOf: Array(token.utf8))
            }
        }
        return String(decoding: bytes, as: UTF8.self)
    }

    private func substitute(_ pattern: NSRegularExpression, in text: String) -> String {
        let boundaryText = String(UnicodeScalar(UInt32(bitPattern: boundary))!)
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
                let key = (Int64(scalar) << 32) | Int64(table[row * 4 + Self.pieceIDColumn])
                suffixID = suffixIDs[key] ?? 0
                if suffixID > 0 || table[row * 4 + Self.scoreColumn] == unknownScore { break }
                row += 1
            }

            row = Int(suffixID)
            while row < rowCount {
                let score = table[row * 4 + Self.scoreColumn]
                if score > invalidScore {
                    let pieceLength = table[row * 4 + Self.pieceLengthColumn]
                    let candidate = scores[index + Int(pieceLength)] - Int64(score)
                    if candidate < scores[index] {
                        scores[index] = candidate
                        pieceLengths[index] = pieceLength
                        tokenIDs[index] = table[row * 4 + Self.tokenIDColumn]
                        tokenCounts[index] = tokenCounts[index + Int(pieceLength)] + 1
                        if score == unknownScore {
                            tokenCounts[index] +=
                                (scalar >= 0x80 ? 1 : 0) + (scalar >= 0x800 ? 1 : 0)
                                + (scalar >= 0x10000 ? 1 : 0)
                        }
                    }
                }
                if score == unknownScore { break }
                row += 1
            }
            index -= 1
        }

        var ids = [Int32]()
        ids.reserveCapacity(Int(tokenCounts[0]))
        var position = 0
        while position < count {
            if tokenIDs[position] >= 0 {
                if tokenIDs[position] != boundaryTokenID { ids.append(tokenIDs[position]) }
            } else {
                let scalar = data[position]
                let byteCount = 1 + (scalar >= 0x80 ? 1 : 0) + (scalar >= 0x800 ? 1 : 0)
                    + (scalar >= 0x10000 ? 1 : 0)
                for part in 0..<byteCount {
                    let lead: Int32 =
                        byteCount == 1 ? scalar : part == 0 ? (0xF00 >> byteCount) & 0xFF : 0x80
                    let shift = Int32((byteCount - part - 1) * 6)
                    ids.append(byteTokens[Int(lead | ((scalar >> shift) & 0x3F))])
                }
            }
            let step = Int(pieceLengths[position])
            position += max(step, 1)
        }
        return ids
    }
}
