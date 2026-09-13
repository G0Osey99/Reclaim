# Reclaim — License & Use Notice

Reclaim is free, open-source software licensed under the **Apache License, Version
2.0** (see [LICENSE](../LICENSE)). Your rights to use, copy, modify, and
redistribute it are governed by that license. This notice restates, in plain
language, the parts that matter before you point a recovery tool at a disk. It is
shown at first run in the app and reproduced in the CLI documentation
(docs/plan/11-safety-legal-licensing.md §5).

## 1. Recover only data you are authorized to access

Recovering data from media you do **not** own, or are **not authorized** to
access, may be **illegal** in your jurisdiction. By using Reclaim you confirm you
have the right to read and recover from the device or image you point it at. You
are solely responsible for how you use it.

## 2. Encryption

Reclaim only reads volumes that are **already unlocked by the operating system**,
or that you unlock by supplying your **own** credentials (e.g. a FileVault
password you know). Reclaim **never attempts to break, brute-force, or bypass**
encryption, and it does not implement its own decryption of FileVault, BitLocker,
LUKS, or VeraCrypt in this release.

## 3. Read-only by design — but no guarantee of recovery

Reclaim opens every source **read-only** and is engineered never to write to the
media it scans. It cannot, however, guarantee that any particular file is
recoverable: blocks that have been overwritten or discarded (TRIM) are gone, and
recovery from failing hardware may be incomplete. Always recover to a **different**
disk, and image failing media first.

## 4. No warranty

As stated in the Apache-2.0 license (Sections 7–8), the software is provided **"AS
IS", without warranties of any kind**, and the authors are **not liable** for any
damages, including lost or unrecoverable data, arising from its use. Data recovery
is inherently uncertain; keep backups.

## 5. Export

Reclaim reads OS-decrypted volumes and does not itself implement cryptographic
decryption in this release. Should cryptographic unlock features be added, the
usual open-source cryptography export considerations (e.g. US EAR 5D002 open-source
notification) will be reviewed at that time.

## 6. Not a certified forensic tool

Reclaim provides deterministic result IDs, an audit log, and per-file hashes, but
it is **not** NIST-CFTT certified. Do not represent its output as certified
forensic evidence.

---

By continuing, you acknowledge that you have read and agree to the Apache-2.0
license and this notice.
