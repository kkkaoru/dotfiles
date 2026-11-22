import Darwin
import Foundation

/// Warms the page cache for a file before it is read. A cold start pays for 478 MB of weights, and
/// `F_RDADVISE` hands that read to the kernel ahead of the first `pread`: the work overlaps whatever
/// runs before the weights are touched, and the read-ahead runs on the disk queue instead of one
/// blocking copy.
public enum Prefetch {
    public static let weightsName = "model.safetensors"

    public static func weights(in directory: URL) {
        warm(directory.appendingPathComponent(weightsName))
    }

    /// Best effort by design: a missing file or a kernel that declines the advice leaves the read
    /// exactly as it was, so this can never fail a start-up.
    public static func warm(_ url: URL) {
        let descriptor = open(url.path, O_RDONLY)
        guard descriptor >= 0 else { return }
        defer { close(descriptor) }
        var info = stat()
        guard fstat(descriptor, &info) == 0, info.st_size > 0 else { return }
        // radvisory counts bytes in an Int32, so a larger file is advised in 1 GiB windows.
        let window: off_t = 1 << 30
        var offset: off_t = 0
        while offset < info.st_size {
            var advice = radvisory(
                ra_offset: offset, ra_count: Int32(min(info.st_size - offset, window)))
            if fcntl(descriptor, F_RDADVISE, &advice) == -1 { return }
            offset += off_t(advice.ra_count)
        }
    }
}
