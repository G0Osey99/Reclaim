# testdata — golden images & recipes

Reclaim's test images are **built, never committed** (docs/plan/09 §2, build guide
Part 3.4). Only the recipes are tracked.

```
testdata/
  recipes/*.toml     # tracked: how to build each golden image
  build/             # generated (gitignored): *.img + *.groundtruth.json + logs
  samples/           # (Phase 1) tracked synthetic signature samples, ≤ 64 KiB each
```

## Build the images

```bash
scripts/gen-images                 # build every recipe
scripts/gen-images exfat-camera-delete   # build one
scripts/gen-images --list
```

Each build produces, under `testdata/build/`:

- `<name>.img` — a raw, partitioned, formatted disk image with real files, some
  deleted per the recipe. Scannable directly: `reclaim info testdata/build/<name>.img`.
- `<name>.groundtruth.json` — the ground truth captured **before** deletion:
  every file's path, size, SHA-256, and physical extents (via `F_LOG2PHYS_EXT`),
  plus which files were deleted. Extents are volume-relative device offsets;
  exFAT/FAT do not expose `F_LOG2PHYS` so their extent lists are empty (the exFAT/
  FAT parsers in Phase 2 recover extents from the filesystem instead).

No sudo is required — images are attached as `CRawDiskImage` disk images and
partitioned with `diskutil`; nothing writes to a real device.

## Recipes shipped (Phase 0)

| Recipe | FS | Scheme | Notes |
|--------|----|--------|-------|
| `exfat-camera-delete` | ExFAT | MBR | DCIM camera layout, 10 photos deleted |
| `fat32-usb-delete`    | MS-DOS FAT32 | MBR | flat layout, 8 files deleted |
| `apfs-delete-history` | APFS | GPT | 12 deleted + 320 create/delete churn (checkpoint history) |
| `hfsplus-delete`      | Journaled HFS+ | GPT | 7 files deleted (catalog slack + journal) |
