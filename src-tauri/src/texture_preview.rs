use base64::{engine::general_purpose::STANDARD, Engine as _};
use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;
use uuid::Uuid;

const ENTRIES_HEADER_SIZE: usize = 44;
const ENTRY_SIZE: usize = 28;
const TEXTURE_HEADER_SIZE: usize = 600;
const MAX_ENTRIES: usize = 1_048_576;
const MAX_INDEX_SIZE: u64 = (ENTRIES_HEADER_SIZE + ENTRY_SIZE * MAX_ENTRIES) as u64;
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_SIDE: u32 = 2048;
const MAX_IMAGE_PIXELS: u64 = 4_194_304;
const MAX_OUTPUT_SIDE: u32 = 480;
const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    index: u32,
    image_size: usize,
    body_size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

struct CachedIndex {
    path: PathBuf,
    stamp: FileStamp,
    entries: HashMap<Uuid, CacheEntry>,
}

static INDEX_CACHE: OnceLock<Mutex<Option<CachedIndex>>> = OnceLock::new();

/// Return a data URL for a single selected inventory item's local Firestorm thumbnail.
/// A cache miss, unsupported format, partial entry, or concurrent write is not an error.
pub fn load_data_url(cache_dir: &Path, thumbnail_id: &str) -> Option<String> {
    let id = Uuid::parse_str(thumbnail_id).ok()?;
    if id.is_nil() {
        return None;
    }
    let cache_root = cache_dir.join("texturecache").canonicalize().ok()?;
    if !cache_root.is_dir() {
        return None;
    }
    let (entry, index_stamp) = cached_entry(&cache_root, id)?;
    let bytes = read_texture(&cache_root, id, entry)?;
    // Firestorm can reuse a texture.cache slot while its index is being read.
    // Reject that read instead of displaying a different asset under this UUID.
    let index_path = readable_child(&cache_root, "texture.entries")?;
    if stamp(&fs::metadata(index_path).ok()?) != index_stamp {
        return None;
    }
    let png = decode_png(&bytes)?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

fn cached_entry(cache_root: &Path, id: Uuid) -> Option<(CacheEntry, FileStamp)> {
    let index_path = cache_root.join("texture.entries").canonicalize().ok()?;
    if !index_path.starts_with(cache_root) {
        return None;
    }
    let stamp = stamp(&fs::metadata(&index_path).ok()?);
    if stamp.len < ENTRIES_HEADER_SIZE as u64 || stamp.len > MAX_INDEX_SIZE {
        return None;
    }

    let cache = INDEX_CACHE.get_or_init(|| Mutex::new(None));
    // Keep the lock while rebuilding: concurrent selections must not each
    // allocate and parse another copy of the same large cache index.
    let mut guard = cache.lock().ok()?;
    if let Some(index) = guard.as_ref() {
        if index.path == index_path && index.stamp == stamp {
            return index.entries.get(&id).copied().map(|entry| (entry, stamp));
        }
    }

    let entries = read_index(&index_path, cache_root, &stamp)?;
    let result = entries.get(&id).copied();
    *guard = Some(CachedIndex {
        path: index_path,
        stamp: stamp.clone(),
        entries,
    });
    result.map(|entry| (entry, stamp))
}

fn read_index(
    path: &Path,
    cache_root: &Path,
    expected_stamp: &FileStamp,
) -> Option<HashMap<Uuid, CacheEntry>> {
    let before = stamp(&fs::metadata(path).ok()?);
    if before != *expected_stamp {
        return None;
    }
    let mut file = File::open(path).ok()?;
    if stamp(&file.metadata().ok()?) != before {
        return None;
    }
    let mut bytes = Vec::with_capacity(before.len as usize);
    (&mut file)
        .take(MAX_INDEX_SIZE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 != before.len || stamp(&file.metadata().ok()?) != before {
        return None;
    }
    if !path.starts_with(cache_root) {
        return None;
    }

    let header = bytes.get(..ENTRIES_HEADER_SIZE)?;
    let version = f32::from_le_bytes(header.get(0..4)?.try_into().ok()?);
    let address_size = u32::from_le_bytes(header.get(4..8)?.try_into().ok()?);
    let encoder = header.get(8..40)?;
    let encoder_end = encoder.iter().position(|byte| *byte == 0)?;
    let count = u32::from_le_bytes(header.get(40..44)?.try_into().ok()?) as usize;
    if version != 1.71
        || address_size != 32
        || encoder_end == 0
        || !encoder[..encoder_end].is_ascii()
        || count > MAX_ENTRIES
    {
        return None;
    }
    let expected_len = ENTRIES_HEADER_SIZE.checked_add(count.checked_mul(ENTRY_SIZE)?)?;
    if bytes.len() < expected_len {
        return None;
    }

    let mut entries = HashMap::with_capacity(count.min(65_536));
    for index in 0..count {
        let offset = ENTRIES_HEADER_SIZE.checked_add(index.checked_mul(ENTRY_SIZE)?)?;
        let raw = bytes.get(offset..offset + ENTRY_SIZE)?;
        let id = Uuid::from_slice(raw.get(..16)?).ok()?;
        let image_size = i32::from_le_bytes(raw.get(16..20)?.try_into().ok()?);
        let body_size = i32::from_le_bytes(raw.get(20..24)?.try_into().ok()?);
        if id.is_nil() || image_size <= 0 || body_size < 0 || image_size <= body_size {
            continue;
        }
        let entry = CacheEntry {
            index: u32::try_from(index).ok()?,
            image_size: usize::try_from(image_size).ok()?,
            body_size: usize::try_from(body_size).ok()?,
        };
        if entries.insert(id, entry).is_some() {
            return None;
        }
    }
    Some(entries)
}

fn read_texture(cache_root: &Path, id: Uuid, entry: CacheEntry) -> Option<Vec<u8>> {
    if entry.image_size == 0
        || entry.image_size > MAX_IMAGE_BYTES
        || entry.body_size != entry.image_size.saturating_sub(TEXTURE_HEADER_SIZE)
    {
        return None;
    }
    let header_path = readable_child(cache_root, "texture.cache")?;
    let header_offset = u64::from(entry.index).checked_mul(TEXTURE_HEADER_SIZE as u64)?;
    let header_end = header_offset.checked_add(TEXTURE_HEADER_SIZE as u64)?;
    let header_file = File::open(&header_path).ok()?;
    let header_before = stamp(&header_file.metadata().ok()?);
    if header_end > header_before.len {
        return None;
    }
    let mut header_file = header_file;
    header_file.seek(SeekFrom::Start(header_offset)).ok()?;
    let mut texture = vec![0; entry.image_size.min(TEXTURE_HEADER_SIZE)];
    let header_bytes = texture.len();
    header_file.read_exact(&mut texture).ok()?;
    if stamp(&header_file.metadata().ok()?) != header_before {
        return None;
    }

    if entry.body_size > 0 {
        let texture_name = id.hyphenated().to_string();
        let relative = PathBuf::from(&texture_name[..1]).join(format!("{texture_name}.texture"));
        let body_path = readable_child(cache_root, &relative)?;
        let body_file = File::open(&body_path).ok()?;
        let body_before = stamp(&body_file.metadata().ok()?);
        if body_before.len != entry.body_size as u64 {
            return None;
        }
        let mut body_file = body_file;
        let mut body = Vec::with_capacity(entry.body_size);
        (&mut body_file)
            .take(entry.body_size as u64 + 1)
            .read_to_end(&mut body)
            .ok()?;
        if body.len() != entry.body_size || stamp(&body_file.metadata().ok()?) != body_before {
            return None;
        }
        if stamp(&fs::metadata(&body_path).ok()?) != body_before {
            return None;
        }
        texture.extend_from_slice(&body);
    } else if header_bytes != entry.image_size {
        return None;
    }

    if stamp(&header_file.metadata().ok()?) != header_before
        || stamp(&fs::metadata(&header_path).ok()?) != header_before
    {
        return None;
    }
    (texture.len() == entry.image_size).then_some(texture)
}

fn readable_child(root: &Path, relative: impl AsRef<Path>) -> Option<PathBuf> {
    let path = root.join(relative).canonicalize().ok()?;
    path.starts_with(root).then_some(path)
}

fn stamp(metadata: &fs::Metadata) -> FileStamp {
    FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
    }
}

fn decode_png(input: &[u8]) -> Option<Vec<u8>> {
    if input.is_empty() || input.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let settings = DecodeSettings {
        target_resolution: None,
        ..DecodeSettings::default()
    };
    let image = Image::new(input, &settings).ok()?;
    let original_pixels = u64::from(image.width()).checked_mul(u64::from(image.height()))?;
    if image.width() == 0
        || image.height() == 0
        || image.width() > MAX_IMAGE_SIDE
        || image.height() > MAX_IMAGE_SIDE
        || original_pixels > MAX_IMAGE_PIXELS
    {
        return None;
    }
    let (color, base_channels) = match image.color_space() {
        ColorSpace::Gray if image.has_alpha() => (png::ColorType::GrayscaleAlpha, 1usize),
        ColorSpace::Gray => (png::ColorType::Grayscale, 1usize),
        ColorSpace::RGB if image.has_alpha() => (png::ColorType::Rgba, 3usize),
        ColorSpace::RGB => (png::ColorType::Rgb, 3usize),
        _ => return None,
    };
    if image.color_space().num_channels() as usize != base_channels {
        return None;
    }
    let width = image.width();
    let height = image.height();
    let source_channels = base_channels + usize::from(image.has_alpha());
    let mut context = DecoderContext::default();
    let decoded = image.decode(&mut context).ok()?;
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    let expected_bytes = usize::try_from(pixels).ok()?.checked_mul(source_channels)?;
    let source = decoded.data_u8();
    if pixels > MAX_IMAGE_PIXELS || source.len() != expected_bytes {
        return None;
    }

    let scale = (f64::from(MAX_OUTPUT_SIDE) / f64::from(width.max(height))).min(1.0);
    let output_width = ((f64::from(width) * scale).floor() as u32).max(1);
    let output_height = ((f64::from(height) * scale).floor() as u32).max(1);
    let output_len = usize::try_from(u64::from(output_width) * u64::from(output_height))
        .ok()?
        .checked_mul(source_channels)?;
    let mut output_pixels = vec![0u8; output_len];
    for output_y in 0..output_height {
        let source_y = (u64::from(output_y) * u64::from(height) / u64::from(output_height)) as u32;
        for output_x in 0..output_width {
            let source_x =
                (u64::from(output_x) * u64::from(width) / u64::from(output_width)) as u32;
            let source_start =
                ((source_y as usize * width as usize) + source_x as usize) * source_channels;
            let output_start =
                ((output_y as usize * output_width as usize) + output_x as usize) * source_channels;
            output_pixels[output_start..output_start + source_channels]
                .copy_from_slice(&source[source_start..source_start + source_channels]);
        }
    }
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, output_width, output_height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&output_pixels).ok()?;
    }
    (output.len() <= MAX_PNG_BYTES).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_bytes(id: Uuid, image_size: i32, body_size: i32) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(ENTRY_SIZE);
        bytes.extend_from_slice(id.as_bytes());
        bytes.extend_from_slice(&image_size.to_le_bytes());
        bytes.extend_from_slice(&body_size.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    fn index_bytes(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(ENTRIES_HEADER_SIZE + entries.len() * ENTRY_SIZE);
        bytes.extend_from_slice(&1.71f32.to_le_bytes());
        bytes.extend_from_slice(&32u32.to_le_bytes());
        let mut encoder = [0u8; 32];
        encoder[..10].copy_from_slice(b"test-j2k-1");
        bytes.extend_from_slice(&encoder);
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for entry in entries {
            bytes.extend_from_slice(entry);
        }
        bytes
    }

    #[test]
    fn parses_only_live_entries_from_the_supported_windows_cache_index() {
        let wanted = Uuid::parse_str("10000000-0000-0000-0000-000000000001").unwrap();
        let empty = Uuid::parse_str("10000000-0000-0000-0000-000000000002").unwrap();
        let bytes = index_bytes(&[entry_bytes(wanted, 900, 300), entry_bytes(empty, 0, 0)]);
        let root = std::env::temp_dir();
        let path = root.join("shelfmark-test-texture.entries");
        fs::write(&path, &bytes).unwrap();
        let stamp = stamp(&fs::metadata(&path).unwrap());
        let parsed = read_index(&path, &root, &stamp).unwrap();
        let entry = parsed.get(&wanted).unwrap();
        assert_eq!(entry.index, 0);
        assert_eq!(entry.image_size, 900);
        assert_eq!(entry.body_size, 300);
        assert!(!parsed.contains_key(&empty));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_unknown_versions_and_truncated_index_records() {
        let id = Uuid::parse_str("20000000-0000-0000-0000-000000000001").unwrap();
        let mut bad_version = index_bytes(&[entry_bytes(id, 600, 0)]);
        bad_version[..4].copy_from_slice(&1.70f32.to_le_bytes());
        let root = std::env::temp_dir();
        let path = root.join("shelfmark-test-invalid.entries");
        fs::write(&path, &bad_version).unwrap();
        let current_stamp = stamp(&fs::metadata(&path).unwrap());
        assert!(read_index(&path, &root, &current_stamp).is_none());

        fs::write(
            &path,
            &index_bytes(&[entry_bytes(id, 600, 0)])[..ENTRIES_HEADER_SIZE + 3],
        )
        .unwrap();
        let stamp = stamp(&fs::metadata(&path).unwrap());
        assert!(read_index(&path, &root, &stamp).is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn decoder_rejects_non_jpeg2000_cache_data() {
        assert!(decode_png(b"not a JPEG2000 image").is_none());
    }

    #[test]
    fn reads_and_decodes_a_small_local_firestorm_cache_thumbnail() {
        let id = Uuid::parse_str("30000000-0000-0000-0000-000000000003").unwrap();
        let image = STANDARD
            .decode("/0//UQAvAAAAAAAIAAAABgAAAAAAAAAAAAAACAAAAAYAAAAAAAAAAAADBwEBBwEBBwEB/1IADAAAAAEAAgQEAAH/XAAKQEBISFBISFD/ZAAlAAFDcmVhdGVkIGJ5IE9wZW5KUEVHIHZlcnNpb24gMi41LjT/kAAKAAAAAABCAAH/k9+AIAlcOVrfgCgGkTpMj8AiCH/H2ggADQL5f6A+EMAI/3+AwfODABvtv6H1AoAMvygR34D/2Q==")
            .unwrap();
        assert!(image.len() < TEXTURE_HEADER_SIZE);

        let test_root = std::env::temp_dir().join(format!("shelfmark-preview-{}", Uuid::new_v4()));
        let cache_root = test_root.join("texturecache");
        fs::create_dir_all(&cache_root).unwrap();
        fs::write(
            cache_root.join("texture.entries"),
            index_bytes(&[entry_bytes(id, image.len() as i32, 0)]),
        )
        .unwrap();
        let mut header = vec![0u8; TEXTURE_HEADER_SIZE];
        header[..image.len()].copy_from_slice(&image);
        fs::write(cache_root.join("texture.cache"), header).unwrap();

        let preview = load_data_url(&test_root, &id.hyphenated().to_string()).unwrap();
        assert!(preview.starts_with("data:image/png;base64,"));
        let png = STANDARD.decode(preview.split_once(',').unwrap().1).unwrap();
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn reconstructs_large_texture_from_the_header_and_uuid_body_file() {
        let id = Uuid::parse_str("30000000-0000-0000-0000-000000000004").unwrap();
        let test_root = std::env::temp_dir().join(format!("shelfmark-body-{}", Uuid::new_v4()));
        let cache_root = test_root.join("texturecache");
        let body_dir = cache_root.join(&id.hyphenated().to_string()[..1]);
        fs::create_dir_all(&body_dir).unwrap();
        fs::write(
            cache_root.join("texture.entries"),
            index_bytes(&[entry_bytes(id, 900, 300)]),
        )
        .unwrap();
        fs::write(
            cache_root.join("texture.cache"),
            vec![0xA5; TEXTURE_HEADER_SIZE],
        )
        .unwrap();
        fs::write(
            body_dir.join(format!("{}.texture", id.hyphenated())),
            vec![0x5A; 300],
        )
        .unwrap();

        let canonical_cache = cache_root.canonicalize().unwrap();
        let (entry, _index_stamp) = cached_entry(&canonical_cache, id).unwrap();
        let image = read_texture(&canonical_cache, id, entry).unwrap();
        assert_eq!(image.len(), 900);
        assert!(image[..TEXTURE_HEADER_SIZE]
            .iter()
            .all(|byte| *byte == 0xA5));
        assert!(image[TEXTURE_HEADER_SIZE..]
            .iter()
            .all(|byte| *byte == 0x5A));
        fs::remove_dir_all(test_root).unwrap();
    }
}
