import Foundation
import MLX
import XCTest

@testable import PlamoCore

/// Manual benchmark, not part of `check.sh`'s verdict: it needs the 501 MB checkpoint and only
/// prints. Run with
/// `PLAMO_LOAD_BENCH=1 swift test --filter LoadBenchTests`.
///
/// Answers one question with numbers: how much of the lukewarm reload's first-eval cost is MLX's
/// own safetensors read, and how does it compare with reading the same bytes with more threads?
final class LoadBenchTests: XCTestCase {
    private var checkpoint: URL {
        let path =
            ProcessInfo.processInfo.environment["PLAMO_MODEL"]
            ?? NSString(string: "~/.local/share/plamo-translate/plamo-3-translate-0.8b-mlx-4bit")
            .expandingTildeInPath
        return URL(fileURLWithPath: path).appendingPathComponent("model.safetensors")
    }

    func testLoadBench() throws {
        guard ProcessInfo.processInfo.environment["PLAMO_LOAD_BENCH"] != nil else { return }
        let url = checkpoint
        let size = (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0
        guard size > 0 else { throw XCTSkip("no checkpoint at \(url.path)") }
        print("checkpoint \(size / 1_000_000) MB")

        for run in 1...3 {
            let t0 = Date()
            let arrays = try loadArrays(url: url)
            let t1 = Date()
            eval(arrays.values)
            let t2 = Date()
            print(
                String(
                    format: "mlx  run %d: load %.2f ms, eval %.2f ms, total %.2f ms",
                    run, t1.timeIntervalSince(t0) * 1_000, t2.timeIntervalSince(t1) * 1_000,
                    t2.timeIntervalSince(t0) * 1_000))
        }

        for threads in [4, 8, 14] {
            let times = (1...3).map { _ in rawRead(url: url, size: size, threads: threads) }
            let best = times.min() ?? 0
            print(
                String(
                    format: "raw  %2d threads: %.2f ms  %.1f GB/s", threads, best * 1_000,
                    Double(size) / best / 1e9))
        }

        // The packed cache: `mmap` plus adoption, so the only remaining work is the eval that
        // faults the pages in.
        let packedURL = Packed.path(beside: url)
        guard FileManager.default.fileExists(atPath: packedURL.path) else {
            print("packed: no cache at \(packedURL.path); run `plamo-swift pack` first")
            return
        }
        for run in 1...3 {
            let t0 = Date()
            let loaded = try Packed.load(url: packedURL)
            let t1 = Date()
            eval(Array(loaded.arrays.values))
            let t2 = Date()
            print(
                String(
                    format: "packed run %d: map+adopt %.2f ms, eval %.2f ms, total %.2f ms", run,
                    t1.timeIntervalSince(t0) * 1_000, t2.timeIntervalSince(t1) * 1_000,
                    t2.timeIntervalSince(t0) * 1_000))
        }
    }

    /// Reads the whole file with `threads` concurrent preads, the way a custom loader would.
    private func rawRead(url: URL, size: Int, threads: Int) -> TimeInterval {
        let fd = open(url.path, O_RDONLY)
        guard fd >= 0 else { return .infinity }
        defer { close(fd) }
        let chunk = 8 << 20
        let bounds = stride(from: 0, to: size, by: chunk).map { ($0, min($0 + chunk, size)) }
        let started = Date()
        let counter = Counter()
        DispatchQueue.concurrentPerform(iterations: threads) { _ in
            let buffer = UnsafeMutableRawPointer.allocate(byteCount: chunk, alignment: 4_096)
            defer { buffer.deallocate() }
            while let index = counter.next(below: bounds.count) {
                let (begin, end) = bounds[index]
                _ = pread(fd, buffer, end - begin, off_t(begin))
            }
        }
        return Date().timeIntervalSince(started)
    }

    /// The next chunk to read, handed out under a lock: a captured `var` is a Swift 6 error.
    private final class Counter: @unchecked Sendable {
        private let lock = NSLock()
        private var value = 0

        func next(below limit: Int) -> Int? {
            lock.lock()
            defer { lock.unlock() }
            guard value < limit else { return nil }
            defer { value += 1 }
            return value
        }
    }
}
