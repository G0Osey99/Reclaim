import Foundation
import ReclaimCore

/// Convenience factories — the generated UniFFI record inits have no defaults.
public enum Options {
    public static func scan(
        quick: Bool = true,
        deep: Bool = true,
        families: [String] = [],
        blockSize: UInt32 = 0,
        bruteForce: Bool = false,
        keepCorrupted: Bool = true,
        unallocatedOnly: Bool = false,
        resume: Bool = false
    ) -> ScanOptions {
        ScanOptions(
            quick: quick,
            deep: deep,
            families: families,
            sigIds: [],
            blockSize: blockSize,
            bruteForce: bruteForce,
            keepCorrupted: keepCorrupted,
            maxFileSize: 4 * 1024 * 1024 * 1024,
            rangeStart: nil,
            rangeEnd: nil,
            unallocatedOnly: unallocatedOnly,
            checkpointSecs: 5,
            resume: resume
        )
    }

    public static func recover(
        dest: String,
        preservePaths: Bool = true,
        flat: Bool = false,
        collision: CollisionPolicy = .rename,
        verify: Bool = true,
        allowSameDevice: Bool = false
    ) -> RecoverOptions {
        RecoverOptions(
            dest: dest,
            preservePaths: preservePaths,
            flat: flat,
            collision: collision,
            verify: verify,
            allowSameDevice: allowSameDevice
        )
    }

    public static func image(
        zstd: Bool = false,
        sparse: Bool = false,
        retries: UInt32 = 3,
        reversePass: Bool = true,
        hash: String = "blake3",
        resume: Bool = false
    ) -> ImageOptions {
        ImageOptions(
            chunkSize: 0,
            retries: retries,
            reversePass: reversePass,
            sparse: sparse,
            zstd: zstd,
            hash: hash,
            resume: resume
        )
    }
}
