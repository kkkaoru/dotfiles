import Foundation
import MLX
import Testing

@testable import PlamoCore

/// The packed cache: it must adopt the same bytes MLX's loader produces, be rebuilt only when the
/// checkpoint changes, and refuse a file it cannot map instead of guessing.
@Suite struct PackedTests {
    private func checkpoint(_ name: String = #function) throws -> URL {
        let directory = try Fixtures.temporaryDirectory("packed")
        try TinyModel.write(to: directory)
        return directory.appendingPathComponent("model.safetensors")
    }

    @Test func adoptsTheSameTensorsAsLoadArrays() throws {
        let url = try checkpoint()
        let packed = try Packed.ensure(checkpoint: url)
        let loaded = try Packed.load(url: packed)

        let expected = try loadArrays(url: url)
        #expect(loaded.arrays.count == expected.count)
        for (name, value) in expected {
            let adopted = try #require(loaded.arrays[name])
            #expect(adopted.shape == value.shape)
            #expect(adopted.dtype == value.dtype)
            #expect((adopted .== value).all().item(Bool.self), "\(name) differs")
        }
    }

    @Test func everyTensorStartsOnAPageBoundary() throws {
        let url = try checkpoint()
        let packed = try Packed.ensure(checkpoint: url)
        let page = Int(getpagesize())
        // The data region starts page-aligned, and so does every slot inside it.
        let header = try #require(FileHandle(forReadingAtPath: packed.path))
        defer { try? header.close() }
        let first = try header.read(upToCount: 8)
        let length = try #require(first)
        let headerLength = length.withUnsafeBytes { $0.loadUnaligned(as: UInt64.self) }
        #expect((8 + Int(headerLength)).isMultiple(of: page))
        let payload = try #require(try header.read(upToCount: Int(headerLength)))
        let json = try JSONSerialization.jsonObject(with: payload) as? [String: Any]
        for (name, value) in try #require(json) where name != "__metadata__" {
            let offsets = try #require((value as? [String: Any])?["data_offsets"] as? [Int])
            #expect(offsets[0].isMultiple(of: page), "\(name) is not aligned")
        }
    }

    @Test func packingTwiceKeepsTheFirstFile() throws {
        let url = try checkpoint()
        let first = try Packed.ensure(checkpoint: url)
        let attributes = try FileManager.default.attributesOfItem(atPath: first.path)
        let second = try Packed.ensure(checkpoint: url)
        #expect(second == first)
        let after = try FileManager.default.attributesOfItem(atPath: second.path)
        #expect(after[.size] as? Int == attributes[.size] as? Int)
        #expect(after[.systemFileNumber] as? Int == attributes[.systemFileNumber] as? Int)
    }

    @Test func aChangedCheckpointIsRepacked() throws {
        let url = try checkpoint()
        _ = try Packed.ensure(checkpoint: url)
        // A newer checkpoint must not be served from the old cache: the stamp carries its mtime.
        try FileManager.default.setAttributes(
            [.modificationDate: Date().addingTimeInterval(60)], ofItemAtPath: url.path)
        #expect(Packed.loadCurrent(checkpoint: url) == nil)
        _ = try Packed.ensure(checkpoint: url)
        let loaded = try #require(Packed.loadCurrent(checkpoint: url))
        #expect(loaded.arrays.isEmpty == false)
    }

    @Test func withoutAPackedFileTheCallerFallsBack() throws {
        let url = try checkpoint()
        #expect(Packed.loadCurrent(checkpoint: url) == nil)
        // The model still loads, from the checkpoint itself.
        let model = try PlamoModel.load(directory: url.deletingLastPathComponent())
        #expect(model.mapping == nil)
    }

    @Test func pinsTheAdoptedPagesInPhysicalMemory() throws {
        let url = try checkpoint()
        let packed = try Packed.ensure(checkpoint: url)
        // A fixture of a few KB is far below any wired-memory limit, so this lock must succeed; a
        // refused lock would leave the weights pageable, which is the failure this pins down.
        let first = try Packed.load(url: packed)
        #expect(first.mapping.locked)
        // The lock goes with the mapping: a second adoption must lock as well, not inherit a stale
        // claim on the same address range.
        let second = try Packed.load(url: packed)
        #expect(second.mapping.locked)
        let name = try #require(first.arrays.keys.first)
        let a = try #require(first.arrays[name])
        let b = try #require(second.arrays[name])
        #expect((a .== b).all().item(Bool.self))
    }

    @Test func theModelAdoptsFromThePackedCache() throws {
        let url = try checkpoint()
        _ = try Packed.ensure(checkpoint: url)
        let model = try PlamoModel.load(directory: url.deletingLastPathComponent())
        #expect(model.mapping != nil)
    }

    @Test func rejectsAFileItCannotAdopt() throws {
        let directory = try Fixtures.temporaryDirectory("packed-bad")
        let cases: [(String, Data)] = [
            ("truncated", Data()),
            ("no tensors", try header(["__metadata__": [String: String]()])),
            (
                "unknown dtype",
                try header(
                    [
                        "w": ["dtype": "F8", "shape": [4], "data_offsets": [0, 16]]
                    ], data: 16)
            ),
            (
                "size mismatch",
                try header(
                    [
                        "w": ["dtype": "F32", "shape": [4], "data_offsets": [0, 8]]
                    ], data: 16)
            ),
            (
                "offsets past the data",
                try header(
                    [
                        "w": ["dtype": "F32", "shape": [4], "data_offsets": [0, 16]]
                    ], data: 8)
            ),
        ]
        for (name, bytes) in cases {
            let url = directory.appendingPathComponent("\(name).packed")
            try bytes.write(to: url)
            #expect(throws: PlamoError.self, "\(name) should be rejected") {
                try Packed.load(url: url)
            }
        }
    }

    @Test func rejectsAMissingFile() {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("absent.packed")
        #expect(throws: PlamoError.self) { try Packed.load(url: url) }
    }

    @Test func refusesAHeaderItCannotRead() throws {
        let directory = try Fixtures.temporaryDirectory("packed-header")
        var badJSON = Data()
        var length = UInt64(3).littleEndian
        badJSON.append(Data(bytes: &length, count: 8))
        badJSON.append(Data("{x}".utf8))
        let cases: [(String, Data)] = [("zero length", Data(count: 8)), ("bad json", badJSON)]
        for (name, bytes) in cases {
            let url = directory.appendingPathComponent("\(name).packed")
            try bytes.write(to: url)
            #expect(throws: PlamoError.self, "\(name) should be rejected") {
                try Packed.load(url: url)
            }
        }
    }

    @Test func refusesToPackAMissingCheckpoint() {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(
            "absent.safetensors")
        // The stamp comes from the checkpoint's own attributes, so the refusal is Foundation's.
        #expect(throws: (any Error).self) { try Packed.ensure(checkpoint: url) }
    }

    /// A safetensors header with `data` bytes of data region, laid out the way `pack` writes one.
    private func header(_ tensors: [String: Any], data: Int = 0) throws -> Data {
        let json = try JSONSerialization.data(withJSONObject: tensors, options: [.sortedKeys])
        var bytes = Data()
        var length = UInt64(json.count).littleEndian
        bytes.append(Data(bytes: &length, count: 8))
        bytes.append(json)
        bytes.append(Data(repeating: 0, count: data))
        return bytes
    }
}
