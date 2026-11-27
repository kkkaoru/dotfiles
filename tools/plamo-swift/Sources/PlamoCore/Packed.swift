import Foundation
import MLX

/// A page-aligned copy of the checkpoint, so the weights are adopted straight out of a file mapping
/// instead of being copied out of the page cache. Packing costs one pass when the model arrives;
/// every reload after that reads nothing: `mmap` is a pointer and each tensor becomes a no-copy
/// Metal buffer over it. Those pages are reclaimable file cache rather than anonymous memory the
/// process owns, which is what an idle server should hold — nothing.
public enum Packed {
    /// The mapping every adopted tensor points into. Its pages go when the last array adopted over
    /// them is destroyed: MLX can still hold a Metal buffer over these pages after the model itself
    /// is released, so unmapping on a schedule of our own would be a use-after-free.
    public final class Mapping {
        internal let base: UnsafeMutableRawPointer
        private let byteCount: Int
        /// Whether the pages are pinned; `munlock` on a range that was never locked would clear a
        /// lock another part of the process holds, so the two calls are paired explicitly.
        internal let locked: Bool

        internal init(base: UnsafeMutableRawPointer, byteCount: Int) {
            self.base = base
            self.byteCount = byteCount
            // The weights are the whole working set of a step: 479 MB read once per token. Pinning
            // them keeps them in physical memory while the machine is under pressure, where a
            // pageable mapping would be evicted and read back from disk inside a decode step. The
            // lukewarm state drops the mapping, which drops the lock, so an idle server holds
            // nothing. Best effort: a refused lock leaves the mapping pageable, as it was.
            self.locked = mlock(base, byteCount) == 0
        }

        deinit {
            if locked { munlock(base, byteCount) }
            munmap(base, byteCount)
        }
    }

    /// The adopted tensors and the mapping they live in.
    public struct Loaded {
        /// One array per tensor, each pointing into `mapping` without a copy.
        public let arrays: [String: MLXArray]
        /// The pages `arrays` point into; they are unmapped when the last of them dies.
        public let mapping: Mapping
    }

    /// The packed file that belongs beside `checkpoint`.
    public static func path(beside checkpoint: URL) -> URL {
        checkpoint.deletingLastPathComponent().appendingPathComponent("model.packed")
    }

    /// Packs `checkpoint` beside it unless that file already matches the checkpoint's size and
    /// modification time. The stamp lives in the safetensors metadata, so the packed file is still
    /// a checkpoint MLX itself can read.
    @discardableResult
    public static func ensure(checkpoint: URL) throws -> URL {
        let target = path(beside: checkpoint)
        let stamp = try stamp(of: checkpoint)
        if let current = try? metadata(url: target), current == stamp { return target }
        try pack(checkpoint: checkpoint, to: target, stamp: stamp)
        return target
    }

    /// The tensors of a packed cache that is still current, or nil when there is none to adopt.
    public static func loadCurrent(checkpoint: URL) -> Loaded? {
        let target = path(beside: checkpoint)
        guard let stamp = try? stamp(of: checkpoint),
            let current = try? metadata(url: target), current == stamp
        else { return nil }
        return try? load(url: target)
    }

    /// Writes `checkpoint` to `target` with every tensor on its own page boundary: `MLXArray(
    /// rawPointer:)` accepts only page-aligned pointers, and a checkpoint packs its tensors end to
    /// end.
    public static func pack(checkpoint: URL, to target: URL, stamp: [String: String]) throws {
        let page = Int(getpagesize())
        let fd = open(checkpoint.path, O_RDONLY)
        guard fd >= 0 else { throw PlamoError.missingModelFile(checkpoint.lastPathComponent) }
        defer { close(fd) }
        let parsed = try header(fd: fd)
        let entries = parsed.entries
        let dataStart = parsed.dataStart

        var slots: [String: Int] = [:]
        var cursor = 0
        for (name, entry) in entries {
            cursor = align(cursor, page)
            slots[name] = cursor
            cursor += align(entry.byteCount, page)
        }
        var object: [String: Any] = ["__metadata__": stamp]
        for (name, entry) in entries {
            let slot = slots[name] ?? 0
            object[name] = [
                "dtype": entry.dtypeName, "shape": entry.shape,
                "data_offsets": [slot, slot + entry.byteCount],
            ]
        }
        // The data region starts at 8 + the header length, so the header is padded with spaces
        // (which JSON ignores) until that lands on a page boundary and every slot is aligned.
        var json = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        while !(8 + json.count).isMultiple(of: page) { json.append(0x20) }

        let temporary = target.deletingLastPathComponent()
            .appendingPathComponent(".\(target.lastPathComponent).\(getpid()).tmp")
        FileManager.default.createFile(atPath: temporary.path, contents: nil)
        let out = open(temporary.path, O_WRONLY | O_TRUNC)
        guard out >= 0 else {
            throw PlamoError.unsupportedModelLayout(reason: "cannot write \(temporary.path)")
        }
        do {
            var length = UInt64(json.count).littleEndian
            try write(out, Data(bytes: &length, count: 8), at: 0)
            try write(out, json, at: 8)
            let placements = entries.map { name, entry in
                Placement(
                    name: name, entry: entry,
                    destination: 8 + json.count + (slots[name] ?? 0))
            }
            try copy(fd: fd, to: out, placements: placements, sourceStart: dataStart)
            close(out)
        } catch {
            close(out)
            try? FileManager.default.removeItem(at: temporary)
            throw error
        }
        guard rename(temporary.path, target.path) == 0 else {
            try? FileManager.default.removeItem(at: temporary)
            throw PlamoError.unsupportedModelLayout(reason: "cannot replace \(target.path)")
        }
    }

    /// The tensors in `url`, each adopted over a slot of the mapping. Throws instead of copying
    /// when a tensor cannot be adopted, so the caller can fall back to the checkpoint's own loader.
    public static func load(url: URL) throws -> Loaded {
        let fd = open(url.path, O_RDONLY)
        guard fd >= 0 else { throw PlamoError.missingModelFile(url.lastPathComponent) }
        defer { close(fd) }
        let parsed = try header(fd: fd)
        // The header resolved every dtype before the mapping exists, so a rejection there leaves
        // nothing to unmap and no array pointing into pages that are about to go.
        let size = Int(lseek(fd, 0, SEEK_END))
        guard size > 0, parsed.entries.values.allSatisfy({ parsed.dataStart + $0.end <= size }),
            let base = mmap(nil, size, PROT_READ, MAP_PRIVATE, fd, 0), base != MAP_FAILED
        else {
            throw PlamoError.unsupportedModelLayout(reason: "cannot map \(url.lastPathComponent)")
        }
        let mapping = Mapping(base: base, byteCount: size)
        var arrays = [String: MLXArray](minimumCapacity: parsed.entries.count)
        for (name, entry) in parsed.entries {
            arrays[name] = MLXArray(
                rawPointer: base + parsed.dataStart + entry.start, entry.shape, dtype: entry.dtype
            ) { _ = mapping }
        }
        return Loaded(arrays: arrays, mapping: mapping)
    }

    /// What the header says about one tensor: its dtype (as written and as MLX sees it), its shape,
    /// and its byte range in the data region.
    private struct Entry {
        let dtypeName: String
        let dtype: DType
        let shape: [Int]
        let start: Int
        let end: Int

        var byteCount: Int { end - start }
    }

    /// Where one tensor's bytes go in the packed file.
    private struct Placement {
        let name: String
        let entry: Entry
        let destination: Int
    }

    /// The parsed header: its tensors, where the data region starts, and the stamp.
    private struct Header {
        let entries: [String: Entry]
        let dataStart: Int
        let metadata: [String: String]
    }

    private static let chunkBytes = 8 << 20

    /// The parsed header, where the data region starts, and the metadata that stamps the file.
    private static func header(fd: Int32) throws -> Header {
        var length = [UInt8](repeating: 0, count: 8)
        guard pread(fd, &length, 8, 0) == 8 else { throw corrupt("header length") }
        let headerLength = length.withUnsafeBytes { $0.loadUnaligned(as: UInt64.self) }
        guard headerLength > 0, headerLength < 1 << 26 else { throw corrupt("header size") }
        var raw = [UInt8](repeating: 0, count: Int(headerLength))
        guard pread(fd, &raw, raw.count, 8) == raw.count else { throw corrupt("header") }
        guard let json = try? JSONSerialization.jsonObject(with: Data(raw)) as? [String: Any] else {
            throw corrupt("header JSON")
        }
        var entries: [String: Entry] = [:]
        for (name, value) in json where name != "__metadata__" {
            guard let object = value as? [String: Any],
                let dtypeName = object["dtype"] as? String,
                let dtype = dtype(forName: dtypeName),
                let shape = numbers(object["shape"]),
                let offsets = numbers(object["data_offsets"]), offsets.count == 2,
                shape.allSatisfy({ $0 >= 0 }), offsets[0] >= 0, offsets[1] >= offsets[0],
                shape.reduce(1, *) * dtype.size == offsets[1] - offsets[0]
            else { throw corrupt(name) }
            entries[name] = Entry(
                dtypeName: dtypeName, dtype: dtype, shape: shape, start: offsets[0], end: offsets[1]
            )
        }
        guard !entries.isEmpty else { throw corrupt("no tensors") }
        let metadata = (json["__metadata__"] as? [String: String]) ?? [:]
        return Header(entries: entries, dataStart: 8 + Int(headerLength), metadata: metadata)
    }

    private static func metadata(url: URL) throws -> [String: String] {
        let fd = open(url.path, O_RDONLY)
        guard fd >= 0 else { throw PlamoError.missingModelFile(url.lastPathComponent) }
        defer { close(fd) }
        return try header(fd: fd).metadata
    }

    /// Identifies the checkpoint a packed file was made from, so a replaced model is repacked
    /// rather than adopted from stale weights.
    private static func stamp(of checkpoint: URL) throws -> [String: String] {
        let attributes = try FileManager.default.attributesOfItem(atPath: checkpoint.path)
        let size = (attributes[.size] as? NSNumber)?.intValue ?? 0
        let mtime = (attributes[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0
        return ["source_size": String(size), "source_mtime": String(format: "%.0f", mtime)]
    }

    /// Copies every tensor's bytes to its slot, one tensor at a time: this runs once per model,
    /// so it stays simple rather than parallel.
    private static func copy(
        fd: Int32, to out: Int32, placements: [Placement], sourceStart: Int
    ) throws {
        let buffer = UnsafeMutableRawPointer.allocate(byteCount: chunkBytes, alignment: 4_096)
        defer { buffer.deallocate() }
        for placement in placements {
            let entry = placement.entry
            let destination = off_t(placement.destination)
            var offset = 0
            while offset < entry.byteCount {
                let length = min(chunkBytes, entry.byteCount - offset)
                guard pread(fd, buffer, length, off_t(sourceStart + entry.start + offset)) == length
                else { throw corrupt("short read of \(placement.name)") }
                guard pwrite(out, buffer, length, destination + off_t(offset)) == length else {
                    throw corrupt("short write of \(placement.name)")
                }
                offset += length
            }
        }
    }

    private static func write(_ fd: Int32, _ data: Data, at offset: Int) throws {
        try data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }
            var done = 0
            while done < raw.count {
                let count = pwrite(fd, base + done, raw.count - done, off_t(offset + done))
                guard count > 0 else { throw corrupt("short write") }
                done += count
            }
        }
    }

    private static func numbers(_ value: Any?) -> [Int]? {
        guard let values = value as? [Any] else { return nil }
        let numbers = values.compactMap { ($0 as? NSNumber)?.intValue }
        return numbers.count == values.count ? numbers : nil
    }

    /// The safetensors dtype names MLX can adopt without a conversion.
    private static func dtype(forName name: String) -> DType? {
        switch name {
        case "BF16": return .bfloat16
        case "F16": return .float16
        case "F32": return .float32
        case "F64": return .float64
        case "U8": return .uint8
        case "U16": return .uint16
        case "U32": return .uint32
        case "I8": return .int8
        case "I16": return .int16
        case "I32": return .int32
        case "I64": return .int64
        default: return nil
        }
    }

    private static func align(_ value: Int, _ page: Int) -> Int {
        (value + page - 1) / page * page
    }

    private static func corrupt(_ field: String) -> PlamoError {
        .unsupportedModelLayout(reason: "packed header: \(field)")
    }
}
