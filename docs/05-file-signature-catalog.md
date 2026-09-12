# 05 — File Signature Catalog & Carving Engine Spec

## 1. Signature definition format

Signatures are data (`reclaim-sigs/catalog/*.toml`), compiled into an Aho-Corasick automaton at build time. Validators/extractors that need code are referenced by `validator = "name"` and live in `reclaim-carve::validators`.

```toml
[[signature]]
id          = "image.jpeg"            # stable, dotted family.format
family      = "image"
extensions  = ["jpg", "jpeg", "jpe"]
mime        = "image/jpeg"
tier        = 1
# One or more header patterns. Bytes in hex; `??` = wildcard. `offset` is where
# the pattern sits relative to file start (default 0).
headers     = [ { pattern = "FF D8 FF E0", offset = 0 },
                { pattern = "FF D8 FF E1", offset = 0 },   # EXIF
                { pattern = "FF D8 FF DB", offset = 0 },
                { pattern = "FF D8 FF EE", offset = 0 } ]
footer      = { pattern = "FF D9", search_max = "256MiB" }
size        = { min = "1KiB", max = "256MiB" }
strategy    = "structural"            # fixed | header_size | footer | structural | statistical
validator   = "jpeg"                  # Rust validator: walks markers, checks SOF/SOS, uses decoder to verify
reassembly  = true                    # participates in fragment reassembly
metadata    = ["exif"]                # extractors for original name/date/thumbnail
block_aligned = true                  # only expect header at cluster boundary in fast path
```

Strategies:
- `fixed` — constant size (e.g. some raw dumps).
- `header_size` — size field parsed from header (BMP, WAV/RIFF, PNG via chunks walk, ELF, Mach-O, SQLite page count × page size, PDF `/Length`… mostly via structural).
- `footer` — scan forward for footer within `search_max`; take smallest valid.
- `structural` — validator walks the format's own structure (chunks/atoms/boxes/markers/records) until logical end; most robust; sets `validity = Truncated` if the media runs out.
- `statistical` — for headerless text: index of coincidence / byte-class histogram over the block; ends when the next block stops looking like text (PhotoRec's approach for `.txt`).

Every result carries `validity ∈ {Full, Truncated, Suspect}` and a `score 0–100` (validator confidence × unallocated-bonus × not-overwritten).

## 2. Priority & de-duplication rules

- When two signatures match the same offset (e.g. `RIFF` → WAV/AVI/WEBP; `PK..` → ZIP/DOCX/XLSX/PPTX/JAR/APK/EPUB/Pages/Numbers/Key), the validator **refines** the id by inspecting the content (`WAVE`/`AVI `/`WEBP` fourcc; `[Content_Types].xml` + `word/`, `xl/`, `ppt/`; `META-INF/MANIFEST.MF`; `mimetype` = `application/epub+zip`; `Index/Document.iwa` for iWork).
- Container-inside-container (JPEG thumbnail inside a CR2, JPEG inside a PDF): the outer structural walk *claims* its byte range; inner matches inside a claimed range are suppressed unless the outer file is `Truncated`/`Suspect`.
- A carved range identical to a metadata-engine extent list collapses into that named entry.

## 3. Seed catalog

Tier 1 = ship in v1.0 with validators; Tier 2 = header/footer only acceptable at launch, validators later; Tier 3 = header-only detection. Hex bytes are at offset 0 unless noted. Abbreviations: BE = big-endian, LE = little-endian.

### 3.1 Images (raster)

| id | ext | Header | End / size | Tier | Notes |
|----|-----|--------|------------|------|-------|
| image.jpeg | jpg | `FF D8 FF (E0|E1|DB|EE|E2|C4)` | `FF D9` + marker walk | 1 | Decode via `image`/`zune-jpeg` to verify; EXIF for date/camera; embedded thumbnail; fragment reassembly §5 |
| image.png | png | `89 50 4E 47 0D 0A 1A 0A` | chunk walk to `IEND` (`49 45 4E 44 AE 42 60 82`), CRC-32 per chunk | 1 | CRCs make validation strong |
| image.gif | gif | `47 49 46 38 (37|39) 61` | block walk to `3B` | 1 | |
| image.bmp | bmp | `42 4D` + size LE @2 | header size | 1 | |
| image.tiff | tif | `49 49 2A 00` (LE) / `4D 4D 00 2A` (BE) | IFD walk; strips/tiles offsets | 1 | Basis of most camera RAWs below |
| image.bigtiff | tif | `49 49 2B 00 08 00` / `4D 4D 00 2B 00 08` | IFD64 walk | 2 | |
| image.webp | webp | `52 49 46 46 ?? ?? ?? ?? 57 45 42 50` | RIFF size @4 | 1 | |
| image.heif | heic/heif/avif | ISOBMFF `ftyp` @4 with brand `heic|heix|hevc|mif1|msf1|avif|avis` | box walk (`ftyp`,`meta`,`mdat`) | 1 | Default iPhone format; preview via libheif |
| image.jp2 | jp2/j2k | `00 00 00 0C 6A 50 20 20 0D 0A 87 0A` / codestream `FF 4F FF 51` | box walk / `FF D9` | 2 | |
| image.psd | psd/psb | `38 42 50 53 00 (01|02)` | section lengths | 1 | Photoshop |
| image.ico | ico/cur | `00 00 (01|02) 00` + count | directory entries | 3 | Weak header; require plausible entries |
| image.tga | tga | footer `TRUEVISION-XFILE.\0` at end | footer | 3 | |
| image.pcx | pcx | `0A (00|02|03|04|05) 01` | — | 3 | |
| image.dds | dds | `44 44 53 20` | header size | 2 | |
| image.exr | exr | `76 2F 31 01` | — | 2 | |
| image.hdr | hdr | `23 3F 52 41 44 49 41 4E 43 45` | — | 3 | |
| image.xcf | xcf | `67 69 6D 70 20 78 63 66 20` | — | 2 | GIMP |
| image.ai_eps | ai/eps | `25 21 50 53 2D 41 64 6F 62 65` (`%!PS-Adobe`) | `%%EOF` | 2 | Modern `.ai` are PDFs |
| image.svg | svg | `3C 3F 78 6D 6C` … `<svg` / `3C 73 76 67` | `</svg>` | 2 | Text; refine from XML |
| image.sketch | sketch | ZIP with `document.json` | ZIP | 2 | |
| image.fig | fig | `fig-kiwi` | — | 3 | Figma |
| image.procreate | procreate | ZIP with `Document.archive` | ZIP | 3 | |
| image.affinity | afphoto/afdesign | `00 FF 4B 41 31 54` | — | 3 | verify |

### 3.2 Camera RAW (all Tier 1 — photographers are the primary persona)

| id | ext | Header | Notes |
|----|-----|--------|-------|
| raw.cr2 | cr2 | `49 49 2A 00 10 00 00 00 43 52` (TIFF + `CR` @8) | Canon; IFD walk; embedded JPEG in IFD0 for preview |
| raw.cr3 | cr3 | ISOBMFF `ftyp` brand `crx ` | Canon R-series; box walk; `CMT1..4` boxes; PRVW/THMB previews |
| raw.crw | crw | `49 49 1A 00 00 00 48 45 41 50 43 43 44 52` | Old Canon CIFF |
| raw.nef | nef/nrw | TIFF BE `4D 4D 00 2A` + `Make=NIKON` in IFD0 | Nikon; distinguish from plain TIFF via Make tag |
| raw.nev | nev | ISOBMFF? (N-RAW video is `.nev` MOV-like) | Nikon N-RAW video — treat as ISOBMFF |
| raw.arw | arw/srf/sr2 | TIFF LE + `Make=SONY` | Sony |
| raw.raf | raf | `46 55 4A 49 46 49 4C 4D 43 43 44 2D 52 41 57` (`FUJIFILMCCD-RAW`) | Fujifilm; header has offsets to JPEG & CFA |
| raw.orf | orf | `49 49 52 4F 08 00 00 00` / `49 49 52 53` / `4D 4D 4F 52` | Olympus/OM System (TIFF variant magic `RO`/`RS`/`OR`) |
| raw.rw2 | rw2 | `49 49 55 00 18 00 00 00 88 E7 74 D8 F8 25 1D 4D 94 7A 6E 77` | Panasonic (TIFF magic `0x55`) |
| raw.pef | pef | TIFF + `Make=PENTAX` / `RICOH` | Pentax |
| raw.dng | dng | TIFF + `DNGVersion` tag (0xC612) | Adobe/iPhone ProRAW/DJI/Leica |
| raw.srw | srw | TIFF + `Make=SAMSUNG` | Samsung |
| raw.3fr | 3fr | TIFF + `Make=Hasselblad` | Hasselblad |
| raw.iiq | iiq | TIFF LE + `49 49 49 49` @8 | Phase One |
| raw.x3f | x3f | `46 4F 56 62` (`FOVb`) | Sigma |
| raw.mrw | mrw | `00 4D 52 4D` | Minolta |
| raw.erf | erf | TIFF + `Make=EPSON` | Epson |
| raw.kdc/dcr | kdc/dcr | TIFF + `Make=KODAK` | Kodak |
| raw.mef | mef | TIFF + `Make=Mamiya` | Mamiya |
| raw.rwl | rwl | same as rw2 | Leica (Panasonic-based) |
| raw.gpr | gpr | DNG variant with VC-5 compression | GoPro |
| raw.ari | ari | `41 52 52 49 12 34 56 78` | ARRIRAW |
| raw.braw | braw | ISOBMFF-ish, `ftyp` brand `braw`? (verify) | Blackmagic RAW |
| raw.r3d | r3d | `?? ?? ?? ?? 52 45 44 31` (`RED1`/`RED2` @4) | RED |

Implementation note: most RAWs are TIFF containers — one `tiff` validator with a `Make`/tag-based refinement table covers ~15 formats. Keep the refinement table in TOML.

### 3.3 Video

| id | ext | Header | End | Tier | Notes |
|----|-----|--------|-----|------|-------|
| video.mp4 | mp4/m4v/m4a/3gp/3g2 | `?? ?? ?? ?? 66 74 79 70` (`ftyp` @4) + brand (`isom`,`iso2`,`mp41`,`mp42`,`avc1`,`M4V `,`M4A `,`3gp*`,`dash`) | box walk; `mdat` size (or 64-bit largesize; size 0 = to EOF) | 1 | Fragment reassembly §5; `moov` may be at end (camera) or start (faststart) |
| video.mov | mov/qt | `ftyp` brand `qt  ` or no ftyp: `moov`/`mdat`/`wide`/`free` as first box | box walk | 1 | iPhone/GoPro/DJI/Canon/Sony all use MOV or MP4 |
| video.avi | avi | `52 49 46 46 ?? ?? ?? ?? 41 56 49 20` | RIFF size (may be >2 GiB via OpenDML `RIFF AVIX`) | 1 | |
| video.mkv | mkv/webm/mka | `1A 45 DF A3` (EBML) | EBML element walk; `webm` DocType | 1 | |
| video.mpeg_ps | mpg/mpeg/vob | `00 00 01 BA` | stream walk / next non-pack | 2 | |
| video.mpeg_ts | ts/m2ts/mts | `47` every 188 bytes (M2TS: 192 with 4-byte TP_extra) | sync-byte continuity | 1 | AVCHD camcorders; detect ≥ 5 consecutive syncs |
| video.mxf | mxf | `06 0E 2B 34 02 05 01 01 0D 01 02 01 01 02` | KLV walk | 2 | Pro cameras (Sony XDCAM, Canon) |
| video.flv | flv | `46 4C 56 01` | tag walk | 2 | |
| video.wmv_asf | wmv/asf/wma | `30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C` | object sizes | 2 | |
| video.rm | rm/rmvb | `2E 52 4D 46` | chunk walk | 3 | |
| video.ogv | ogv/ogg | `4F 67 67 53` | page walk | 2 | |
| video.prproj/fcpbundle | — | ZIP/gzip XML; FCP library = directory | — | 3 | Project files |
| video.insv/insp | insv/insp | MP4-based (Insta360) with trailer | box walk + trailer | 2 | Brand-specific trailer metadata |
| video.360 | 360 | MP4-based (GoPro) | box walk | 2 | |
| video.lrv/thm | lrv/thm | MP4 / JPEG | — | 1 | GoPro proxies |

### 3.4 Audio

| id | ext | Header | Tier | Notes |
|----|-----|--------|------|-------|
| audio.wav | wav | `RIFF....WAVE` | 1 | RIFF size; RF64 `52 46 36 34` |
| audio.aiff | aif/aiff/aifc | `46 4F 52 4D ?? ?? ?? ?? 41 49 46 (46|43)` | 1 | IFF chunk walk |
| audio.mp3 | mp3 | `49 44 33` (ID3v2) or frame sync `FF (FB|FA|F3|F2|E3)` | 1 | Frame-walk to validate; ID3v1 `TAG` footer at end |
| audio.flac | flac | `66 4C 61 43` | 1 | Metadata blocks + frame sync `FF F8` |
| audio.aac_adts | aac | `FF F1` / `FF F9` | 2 | ADTS frame walk |
| audio.ogg | ogg/oga/opus | `4F 67 67 53` | 1 | Refine Vorbis/Opus/Theora/FLAC |
| audio.m4a | m4a | ISOBMFF `M4A ` | 1 | via video.mp4 |
| audio.caf | caf | `63 61 66 66 00 01 00 00` | 2 | Apple Core Audio |
| audio.ape | ape | `4D 41 43 20` | 3 | |
| audio.wv | wv | `77 76 70 6B` | 3 | WavPack |
| audio.mid | mid | `4D 54 68 64` | 2 | |
| audio.amr | amr | `23 21 41 4D 52` | 3 | |
| audio.dsf/dff | dsf/dff | `44 53 44 20` / `46 52 4D 38` | 3 | |
| audio.logic | logicx | Package dir / `Alternatives/…/ProjectData` | 3 | |
| audio.gband | band | Package dir | 3 | GarageBand |
| audio.als | als | gzip (`1F 8B`) containing XML `Ableton` | 3 | |

### 3.5 Documents & office

| id | ext | Header | Tier | Notes |
|----|-----|--------|------|-------|
| doc.pdf | pdf | `25 50 44 46 2D` (`%PDF-`) | 1 | Walk to last `%%EOF` (multiple possible; incremental updates); xref sanity; `Truncated` if none |
| doc.ooxml | docx/xlsx/pptx | `50 4B 03 04` + `[Content_Types].xml` | 1 | ZIP walk to EOCD `50 4B 05 06`; refine by `word/`, `xl/`, `ppt/`; `docProps/core.xml` for title/dates |
| doc.ole2 | doc/xls/ppt/msg/vsd | `D0 CF 11 E0 A1 B1 1A E1` | 1 | Parse FAT/DIFAT to compute size (PhotoRec does this); refine by stream names (`WordDocument`, `Workbook`, `PowerPoint Document`, `__properties_version1.0` → .msg) |
| doc.rtf | rtf | `7B 5C 72 74 66` (`{\rtf`) | 1 | brace-balance to end |
| doc.odf | odt/ods/odp | ZIP with `mimetype` = `application/vnd.oasis.opendocument.*` | 2 | |
| doc.iwork | pages/numbers/key | ZIP with `Index/Document.iwa` (new) or `index.xml` (legacy) / package dir | 1 | macOS-critical |
| doc.epub | epub | ZIP, `mimetype`=`application/epub+zip` | 2 | |
| doc.mobi | mobi/azw | `BOOKMOBI` @60 | 3 | PalmDB header |
| doc.djvu | djvu | `41 54 26 54 46 4F 52 4D ?? ?? ?? ?? 44 4A 56` | 3 | |
| doc.chm | chm | `49 54 53 46 03 00 00 00` | 3 | |
| doc.xps | xps/oxps | ZIP + `FixedDocumentSequence.fdseq` | 3 | |
| doc.txt | txt/md/csv/log | none — statistical | 1 | UTF-8/16 BOM `EF BB BF`/`FF FE`/`FE FF` as hints; index of coincidence + printable ratio; language-agnostic |
| doc.html | html/htm | `3C 21 44 4F 43 54 59 50 45` / `3C 68 74 6D 6C` (case-insens.) | 2 | to `</html>` |
| doc.xml | xml/plist/svg/… | `3C 3F 78 6D 6C` | 2 | refine by root element (`plist`, `svg`, `kml`, `gpx`, `opml`) |
| doc.json | json | `7B` / `5B` + statistical | 3 | weak |
| doc.bplist | plist | `62 70 6C 69 73 74 30 30` (`bplist00`) | 1 | Apple binary plist; trailer gives size |
| doc.ics/vcf | ics/vcf | `BEGIN:VCALENDAR` / `BEGIN:VCARD` | 2 | to `END:…` |
| doc.tex | tex | statistical + `\documentclass` | 3 | |
| doc.notes | — | SQLite (`NoteStore.sqlite`) | 2 | via database.sqlite |
| doc.onenote | one | `E4 52 5C 7B 8C D8 A7 4D AE B1 53 78 D0 29 96 D3` | 3 | |

### 3.6 Archives & compression

| id | ext | Header | Tier | Notes |
|----|-----|--------|------|-------|
| archive.zip | zip/jar/apk/ipa/xpi | `50 4B 03 04` (also empty `50 4B 05 06`, spanned `50 4B 07 08`) | 1 | Local-header walk + central dir; ZIP64; refine sub-types |
| archive.rar | rar | `52 61 72 21 1A 07 00` (v1.5–4) / `52 61 72 21 1A 07 01 00` (v5) | 1 | block walk; end-archive block |
| archive.7z | 7z | `37 7A BC AF 27 1C` | 1 | start header has end-header offset+size → exact length |
| archive.gzip | gz/tgz | `1F 8B 08` | 1 | deflate stream walk (inflate to find end); ISIZE trailer |
| archive.bzip2 | bz2 | `42 5A 68 (31-39)` | 1 | block magic `31 41 59 26 53 59`, end `17 72 45 38 50 90` |
| archive.xz | xz | `FD 37 7A 58 5A 00` | 1 | footer `59 5A` |
| archive.zstd | zst | `28 B5 2F FD` | 1 | frame walk |
| archive.lz4 | lz4 | `04 22 4D 18` | 2 | |
| archive.tar | tar | `75 73 74 61 72` (`ustar`) @257 | 1 | header checksum validation; 512-byte record walk; two zero blocks end |
| archive.cab | cab | `4D 53 43 46` | 2 | size @8 |
| archive.arj/lzh/ace | — | `60 EA` / `-lh?-` @2 / `**ACE**` @7 | 3 | |
| archive.dmg | dmg | `koly` trailer (`6B 6F 6C 79`) 512 B from end; UDIF | 1 | Footer-anchored: scan for `koly`, read data-fork length → compute start. Also compressed-block signatures |
| archive.sparsebundle | — | directory of `bands/` | — | via FS metadata only |
| archive.xar/pkg | xar/pkg | `78 61 72 21` | 1 | macOS installers |
| archive.iso | iso | `CD001` @32769 | 1 | ISO9660 PVD → volume space size × 2048 |
| archive.deb/rpm | deb/rpm | `21 3C 61 72 63 68 3E` / `ED AB EE DB` | 3 | |
| archive.stuffit | sit/sitx | `53 49 54 21` / `53 74 75 66 66 49 74` | 3 | Vintage Mac |

### 3.7 Databases, mail, app data

| id | ext | Header | Tier | Notes |
|----|-----|--------|------|-------|
| db.sqlite | sqlite/db (Photos.sqlite, Messages chat.db, Safari History.db, Notes) | `53 51 4C 69 74 65 20 66 6F 72 6D 61 74 20 33 00` | 1 | page size @16, page count @28 → exact size; WAL `37 7F 06 82`/`37 7F 06 83` files too |
| db.realm | realm | `54 2D 44 42` (`T-DB`) @? (verify) | 3 | |
| db.leveldb | ldb/log | `57 FB 80 8B 24 75 47 DB` footer | 3 | Chrome/Electron data |
| db.mdb/accdb | mdb/accdb | `00 01 00 00 53 74 61 6E 64 61 72 64 20 (4A 65 74|41 43 45) 20 44 42` | 2 | |
| db.pst/ost | pst | `21 42 44 4E` (`!BDN`) | 2 | |
| mail.mbox | mbox | `46 72 6F 6D 20` (`From `) + RFC 5322 headers | 2 | statistical |
| mail.emlx | emlx | decimal length line + headers | 2 | Apple Mail |
| mail.eml | eml | `Received:` / `Return-Path:` / `From:` at line start | 2 | |
| app.keychain | keychain-db | SQLite (modern) / `6B 79 63 68` (legacy) | 2 | never decrypt — just recover file |
| app.safari_webarchive | webarchive | bplist root `WebMainResource` | 2 | |
| app.ibooks/plist | — | bplist | — | |
| app.xcode | xcodeproj | dir / `// !$*UTF8*$!` pbxproj | 2 | text |
| app.git | — | `PACK` objects, zlib loose objects | 3 | Recovering repos is a real developer scenario: pack files `50 41 43 4B 00 00 00 02` |

### 3.8 Executables, code, system

| id | ext | Header | Tier |
|----|-----|--------|------|
| exec.macho | — | `FE ED FA CE` / `FE ED FA CF` / `CE FA ED FE` / `CF FA ED FE`; fat `CA FE BA BE` | 1 (load-command walk for size) |
| exec.elf | — | `7F 45 4C 46` | 1 |
| exec.pe | exe/dll | `4D 5A` + `PE\0\0` at e_lfanew | 1 |
| exec.class/jar | class | `CA FE BA BE` (disambiguate from fat Mach-O by version field) | 2 |
| exec.wasm | wasm | `00 61 73 6D` | 2 |
| exec.dex | dex | `64 65 78 0A 30 33 35 00` | 3 |
| exec.pyc | pyc | version-specific magic | 3 |
| exec.shell_script | sh/py/rb | `23 21` (`#!`) + statistical | 2 |
| sys.plist_xml | plist | XML + `<!DOCTYPE plist` | 1 |
| sys.dyld_cache | — | `dyld_v1` | 3 |

### 3.9 Disk images, VM & crypto containers

| id | ext | Header | Tier |
|----|-----|--------|------|
| disk.vmdk | vmdk | `4B 44 4D 56` (`KDMV`) sparse / `# Disk DescriptorFile` | 2 |
| disk.vdi | vdi | `3C 3C 3C 20 (Oracle|Sun|innotek) VM VirtualBox Disk Image >>>` | 2 |
| disk.vhd | vhd | `conectix` footer | 2 |
| disk.vhdx | vhdx | `76 68 64 78 66 69 6C 65` | 2 |
| disk.qcow2 | qcow2 | `51 46 49 FB` | 2 |
| disk.e01 | E01 | `45 56 46 09 0D 0A FF 00` | 2 |
| disk.utm/parallels | hdd | `57 69 74 68 6F 75 74 46 72 65 65 53 70 61 63 65` (Parallels) | 3 |
| crypto.luks | — | `4C 55 4B 53 BA BE` | 2 |
| crypto.bitlocker | — | `EB 58 90 2D 46 56 45 2D 46 53 2D` (`-FVE-FS-`) | 2 |
| crypto.pgp/asc | gpg/asc | `-----BEGIN PGP` / packet tag | 3 |
| crypto.pem/der | pem/der/p12 | `-----BEGIN` / `30 82` | 2 (DER length gives size) |
| crypto.ssh_key | — | `-----BEGIN OPENSSH PRIVATE KEY-----` | 2 |

### 3.10 3D, CAD, fonts, misc

| id | ext | Header | Tier |
|----|-----|--------|------|
| model.stl | stl | `solid ` (ASCII) / binary: 80-byte header + triangle count → size = 84 + 50n | 1 (binary size is exact) |
| model.3mf | 3mf | ZIP + `3D/3dmodel.model` | 2 |
| model.obj | obj | statistical + `v ` / `f ` lines | 3 |
| model.fbx | fbx | `4B 61 79 64 61 72 61 20 46 42 58 20 42 69 6E 61 72 79` | 2 |
| model.gltf/glb | glb | `67 6C 54 46` | 2 |
| model.blend | blend | `42 4C 45 4E 44 45 52` | 2 |
| model.usdz/usd | usdz/usdc | ZIP (uncompressed) / `50 58 52 2D 55 53 44 43` | 2 |
| model.f3d/step/dwg | — | ZIP / `ISO-10303-21` / `41 43 31 30` | 3 |
| slicer.gcode/3mf | gcode | `;FLAVOR` / `; generated by` + statistical | 3 |
| font.ttf/otf | ttf/otf | `00 01 00 00` / `4F 54 54 4F`; `74 72 75 65` | 2 (table directory gives size) |
| font.woff/2 | woff | `77 4F 46 (46|32)` | 2 |
| geo.gpx/kml | gpx/kml | XML refine | 2 |
| geo.fit | fit | `.FIT` @8 | 2 (header gives size) — fitness devices |
| misc.torrent | torrent | `64 38 3A 61 6E 6E 6F 75 6E 63 65` | 3 |
| misc.swf | swf | `46 57 53` / `43 57 53` | 3 |
| misc.lnk | lnk | `4C 00 00 00 01 14 02 00` | 3 |
| misc.ds_store | .DS_Store | `00 00 00 01 42 75 64 31` | 3 (filter noise) |

Target count: ~150 at launch, ~400 by 1.x. Maintain the catalog as the community-facing asset; accept signature PRs with a test sample.

## 4. Validators worth writing early (biggest false-positive reducers)

1. **ISOBMFF box walker** (MP4/MOV/HEIC/CR3/M4A): validates box sizes, knows `mdat` may precede `moov`, handles 64-bit sizes and size-0-to-EOF.
2. **TIFF IFD walker** (TIFF + ~15 RAW formats): follows IFD chain, computes max extent of all strips/tiles/sub-IFDs, reads `Make`/`Model`/`DateTimeOriginal`.
3. **JPEG marker walker**: SOI → APPn/DQT/SOF/DHT/SOS → entropy-coded scan (skip to next `FF xx` where xx ∉ {00, D0–D7}) → EOI; optionally decode first MCU rows.
4. **ZIP walker** + sub-type refinement (covers ~20 formats).
5. **OLE2 FAT walker** (old Office, Outlook msg).
6. **PDF**: header → last `%%EOF` within limits; check `startxref` pointer resolves.
7. **RIFF/IFF walker** (WAV/AVI/WEBP/AIFF).
8. **SQLite**: header fields → exact size; page 1 B-tree sanity.
9. **Text classifier**: byte histogram, UTF-8 validity, index of coincidence; classify language-agnostic; detect CSV/JSON/source-code sub-type cheaply.
10. **DMG koly** footer-anchored reverse carving.

## 5. Fragment reassembly (v1.x flagship feature)

### 5.1 Why
Cameras write video and burst photos while the FAT/exFAT allocator is interleaving other files; deleted-then-partially-overwritten cards produce fragments. Contiguous carving yields truncated/corrupt files; metadata recovery fails when the chain is gone. This is the "fragmented video reconstruction" competitors market.

### 5.2 JPEG (bifragment + multi-fragment)
1. Carve header fragment H (SOI … into scan data). Decode progressively; the decoder reports the first MCU row where data becomes invalid/garbage → fragmentation point P (cluster-aligned).
2. Candidate continuation clusters: unallocated clusters not starting with a known header, with entropy consistent with JPEG entropy-coded data (few `FF 00` stuffing bytes present, no `FF D8`).
3. For each candidate c within a window (±N MiB of P, expanding): splice H[0..P] + c…, decode; score by decoder error position and **visual continuity** (row-to-row pixel difference at the splice boundary, as in Garfinkel's/Pal-Memon SmartCarving work). Greedy best-first; stop when EOI reached and decode clean.
4. Multi-fragment: repeat from the new failure point. Cap at K fragments (e.g. 8).

### 5.3 MP4/MOV
1. Find `moov` (often at end for cameras; `mdat` first). Parse `stco/co64` (chunk offsets), `stsz` (sample sizes), `stsc`, `stts` — this gives the **exact expected layout** of `mdat` as a sequence of chunk offsets & sizes relative to file start.
2. If `moov` found and `mdat` header found: the chunk table is a map of which file-relative offsets must contain which chunk. For each chunk, verify plausibility at the expected cluster (H.264/HEVC NAL start codes or length-prefixed NALs with sane types); when a chunk is missing/overwritten, search nearby unallocated clusters for a NAL sequence that continues the previous chunk (frame numbers / POC continuity via slice header parse).
3. If `moov` missing (camera died mid-record): reconstruct from `mdat` only — walk length-prefixed NALs, group into frames by AUD/IDR, infer fps from SPS/VUI or brand default, synthesize a `moov` (this is what GoPro/Sony "repair" tools do). Output flagged `Repaired`.
4. Match orphaned `moov` atoms to orphaned `mdat` runs by duration × bitrate ≈ size and by `creation_time`.

### 5.4 Others
- PNG/ZIP/7z: per-chunk CRCs let you detect the exact bad cluster and search for the replacement; lower priority.
- Office/PDF: generally not worth it in v1.x.

### 5.5 Data model
`fragments` table: (result_id, seq, offset, len, confidence). Recovery concatenates in seq order; report shows fragment count.

## 6. Metadata extraction for naming

When the metadata engine gives no name, synthesize: `{family}/{yyyy-mm-dd}/{camera-model or sub-type}/{original-name-if-found or f{offset:016x}}.{ext}`. Sources: EXIF `DateTimeOriginal`/`Model`/`ImageDescription`, XMP `dc:title`, ID3 title/artist, `docProps/core.xml`, PDF `/Title`, ISOBMFF `©nam`/`creation_time`, ZIP internal filenames (e.g. `.sketch` has `document.json` → name in `meta.json`).
