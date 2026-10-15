//! Lossless container privacy filtering. Pixel/animation payloads are retained.
//! GIF89a: https://www.w3.org/Graphics/GIF/spec-gif89a.txt
//! WebP: https://developers.google.com/speed/webp/docs/riff_container

type CleanResult = Result<Vec<u8>, &'static str>;
const MALFORMED: &str = "image container is malformed; upload was not stored";

fn take(data: &[u8], start: usize, length: usize) -> Result<&[u8], &'static str> {
    data.get(start..start.checked_add(length).ok_or(MALFORMED)?)
        .ok_or(MALFORMED)
}
fn le16(data: &[u8]) -> u32 {
    u16::from_le_bytes([data[0], data[1]]) as u32
}
fn le24(data: &[u8]) -> u32 {
    u32::from_le_bytes([data[0], data[1], data[2], 0])
}
fn le32(data: &[u8]) -> u32 {
    u32::from_le_bytes(data[..4].try_into().unwrap())
}

pub(super) fn jpeg(data: &[u8]) -> CleanResult {
    if take(data, 0, 2)? != [0xff, 0xd8] {
        return Err(MALFORMED);
    }
    let mut output = data[..2].to_vec();
    let mut offset = 2;
    let mut saw_scan = false;
    while offset < data.len() {
        let start = offset;
        if data[offset] != 0xff {
            return Err(MALFORMED);
        }
        while data.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let marker = take(data, offset, 1)?[0];
        offset += 1;
        if marker == 0xd9 {
            if !saw_scan {
                return Err(MALFORMED);
            }
            output.extend_from_slice(&[0xff, 0xd9]);
            return Ok(output); // Discard non-image trailing payload.
        }
        if matches!(marker, 0x00 | 0xd8) {
            return Err(MALFORMED);
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            output.extend_from_slice(&data[start..offset]);
            continue;
        }
        let header = take(data, offset, 2)?;
        let length = u16::from_be_bytes([header[0], header[1]]) as usize;
        if length < 2 {
            return Err(MALFORMED);
        }
        let segment = take(data, offset, length)?;
        let payload = &segment[2..];
        match marker {
            0xe0 if payload.starts_with(b"JFIF\0") => {
                // Preserve color/density interpretation, omit the thumbnail.
                if payload.len() < 14
                    || payload.len() != 14 + 3 * payload[12] as usize * payload[13] as usize
                {
                    return Err(MALFORMED);
                }
                output.extend_from_slice(&[0xff, 0xe0, 0, 16]);
                output.extend_from_slice(&payload[..12]);
                output.extend_from_slice(&[0, 0]);
            }
            0xe2 if payload.starts_with(b"ICC_PROFILE\0") => {
                if payload.len() < 14 || payload[12] == 0 || payload[13] < payload[12] {
                    return Err(MALFORMED);
                }
                output.extend_from_slice(&data[start..offset + length]);
            }
            0xee if payload.starts_with(b"Adobe") && payload.len() == 12 => {
                output.extend_from_slice(&data[start..offset + length]);
            }
            0xe0..=0xef | 0xfe => {} // Other application, thumbnail, comment data.
            _ => output.extend_from_slice(&data[start..offset + length]),
        }
        offset += length;
        if marker == 0xda {
            saw_scan = true;
            let entropy = offset;
            while offset < data.len() {
                if data[offset] != 0xff {
                    offset += 1;
                    continue;
                }
                let next = take(data, offset, 2)?[1];
                if next == 0 || (0xd0..=0xd7).contains(&next) {
                    offset += 2;
                    continue;
                }
                break;
            }
            output.extend_from_slice(&data[entropy..offset]);
        }
    }
    Err(MALFORMED)
}

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 0 {
                    value >> 1
                } else {
                    0xedb88320 ^ (value >> 1)
                };
            }
            value
        })
    });
    let mut value = u32::MAX;
    for byte in data {
        value = table[((value as u8) ^ byte) as usize] ^ (value >> 8);
    }
    !value
}

pub(super) fn png(data: &[u8]) -> CleanResult {
    if take(data, 0, 8)? != b"\x89PNG\r\n\x1a\n" {
        return Err(MALFORMED);
    }
    let mut output = data[..8].to_vec();
    let mut offset = 8;
    let mut image = false;
    while offset < data.len() {
        let header = take(data, offset, 8)?;
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let chunk = take(data, offset, length.checked_add(12).ok_or(MALFORMED)?)?;
        let kind = &header[4..];
        if !kind.iter().all(u8::is_ascii_alphabetic) || kind[2] & 32 != 0 {
            return Err(MALFORMED);
        }
        if crc32(&chunk[4..8 + length])
            != u32::from_be_bytes(chunk[8 + length..].try_into().unwrap())
        {
            return Err(MALFORMED);
        }
        if offset == 8 && (kind != b"IHDR" || length != 13) {
            return Err(MALFORMED);
        }
        if kind == b"IHDR" {
            if offset != 8 || length != 13 {
                return Err(MALFORMED);
            }
            let width = u32::from_be_bytes(chunk[8..12].try_into().unwrap());
            let height = u32::from_be_bytes(chunk[12..16].try_into().unwrap());
            if width == 0
                || height == 0
                || width > i32::MAX as u32
                || height > i32::MAX as u32
                || chunk[18] != 0
                || chunk[19] != 0
                || chunk[20] > 1
            {
                return Err(MALFORMED);
            }
        }
        let keep = matches!(
            kind,
            b"IHDR"
                | b"PLTE"
                | b"IDAT"
                | b"IEND"
                | b"tRNS"
                | b"gAMA"
                | b"cHRM"
                | b"sRGB"
                | b"iCCP"
                | b"sBIT"
                | b"bKGD"
                | b"pHYs"
                | b"acTL"
                | b"fcTL"
                | b"fdAT"
                | b"cICP"
                | b"mDCv"
                | b"cLLi"
        );
        if !keep && kind[0] & 32 == 0 {
            return Err(MALFORMED);
        }
        if keep {
            output.extend_from_slice(chunk);
        }
        if kind == b"IDAT" && length > 0 {
            image = true;
        }
        if kind == b"IEND" {
            if length != 0 || !image {
                return Err(MALFORMED);
            }
            return Ok(output);
        }
        offset += chunk.len();
    }
    Err(MALFORMED)
}

fn gif_blocks(data: &[u8], offset: &mut usize) -> Result<usize, &'static str> {
    let mut payload = 0;
    loop {
        let length = take(data, *offset, 1)?[0] as usize;
        *offset += 1;
        if length == 0 {
            return Ok(payload);
        }
        take(data, *offset, length)?;
        *offset += length;
        payload += length;
    }
}

pub(super) fn gif(data: &[u8]) -> CleanResult {
    let header = take(data, 0, 13)?;
    if !matches!(&header[..6], b"GIF87a" | b"GIF89a")
        || le16(&header[6..]) == 0
        || le16(&header[8..]) == 0
    {
        return Err(MALFORMED);
    }
    let table_size = |flags: u8| {
        if flags & 128 == 0 {
            0
        } else {
            3usize << ((flags & 7) + 1)
        }
    };
    let mut offset = 13 + table_size(header[10]);
    let mut output = take(data, 0, offset)?.to_vec();
    let mut image = false;
    loop {
        let start = offset;
        let kind = take(data, offset, 1)?[0];
        offset += 1;
        match kind {
            0x3b => {
                if !image {
                    return Err(MALFORMED);
                }
                output.push(0x3b);
                return Ok(output);
            }
            0x2c => {
                let descriptor = take(data, offset, 9)?;
                if le16(&descriptor[4..]) == 0 || le16(&descriptor[6..]) == 0 {
                    return Err(MALFORMED);
                }
                offset += 9 + table_size(descriptor[8]);
                let code_size = take(data, offset, 1)?[0];
                if !(2..=8).contains(&code_size) {
                    return Err(MALFORMED);
                }
                offset += 1;
                if gif_blocks(data, &mut offset)? == 0 {
                    return Err(MALFORMED);
                }
                output.extend_from_slice(&data[start..offset]);
                image = true;
            }
            0x21 => {
                let label = take(data, offset, 1)?[0];
                offset += 1;
                let extension_start = offset;
                let first_size = take(data, offset, 1)?[0] as usize;
                let first = take(data, offset + 1, first_size)?;
                gif_blocks(data, &mut offset)?;
                let keep = match label {
                    0xf9 => {
                        if first_size != 4 || offset - extension_start != 6 {
                            return Err(MALFORMED);
                        }
                        true
                    }
                    0x01 => {
                        if first_size != 12 {
                            return Err(MALFORMED);
                        }
                        true // Rendered text is visible image content.
                    }
                    0xff => {
                        if first_size != 11 {
                            return Err(MALFORMED);
                        }
                        if first == b"NETSCAPE2.0" || first == b"ANIMEXTS1.0" {
                            // Only the actual loop control, never arbitrary tail data.
                            let controls = &data[extension_start + 12..offset];
                            if controls.len() != 5
                                || controls[0] != 3
                                || controls[1] != 1
                                || controls[4] != 0
                            {
                                return Err(MALFORMED);
                            }
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                };
                if keep {
                    output.extend_from_slice(&data[start..offset]);
                }
            }
            _ => return Err(MALFORMED),
        }
    }
}

fn riff_chunk<'a>(
    data: &'a [u8],
    offset: &mut usize,
) -> Result<(&'a [u8], &'a [u8]), &'static str> {
    let header = take(data, *offset, 8)?;
    let length = le32(&header[4..]) as usize;
    let total = 8usize
        .checked_add(length)
        .and_then(|n| n.checked_add(length & 1))
        .ok_or(MALFORMED)?;
    let chunk = take(data, *offset, total)?;
    if length & 1 != 0 && chunk[total - 1] != 0 {
        return Err(MALFORMED);
    }
    *offset += total;
    Ok((&header[..4], &chunk[8..8 + length]))
}
fn push_chunk(output: &mut Vec<u8>, kind: &[u8], payload: &[u8]) {
    output.extend_from_slice(kind);
    output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    output.extend_from_slice(payload);
    if payload.len() & 1 != 0 {
        output.push(0);
    }
}
fn image_dimensions(kind: &[u8], payload: &[u8]) -> Result<(u32, u32), &'static str> {
    if kind == b"VP8L" {
        take(payload, 0, 5)?;
        if payload[0] != 0x2f || payload[4] & 0xe0 != 0 {
            return Err(MALFORMED);
        }
        let bits = le32(&payload[1..]);
        Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
    } else {
        take(payload, 0, 10)?;
        if payload[0] & 1 != 0 || &payload[3..6] != b"\x9d\x01\x2a" {
            return Err(MALFORMED);
        }
        let width = le16(&payload[6..]) & 0x3fff;
        let height = le16(&payload[8..]) & 0x3fff;
        if width == 0 || height == 0 {
            return Err(MALFORMED);
        }
        Ok((width, height))
    }
}
fn webp_frame(data: &[u8], expected: (u32, u32)) -> CleanResult {
    let mut offset = 0;
    let mut output = Vec::with_capacity(data.len());
    let mut alpha = false;
    let mut image = false;
    while offset < data.len() {
        let (kind, payload) = riff_chunk(data, &mut offset)?;
        match kind {
            b"ALPH" => {
                if alpha || image || payload.is_empty() {
                    return Err(MALFORMED);
                }
                alpha = true;
                push_chunk(&mut output, kind, payload);
            }
            b"VP8 " | b"VP8L" => {
                if image
                    || (kind == b"VP8L" && alpha)
                    || image_dimensions(kind, payload)? != expected
                {
                    return Err(MALFORMED);
                }
                image = true;
                push_chunk(&mut output, kind, payload);
            }
            b"ANMF" | b"ANIM" | b"VP8X" | b"ICCP" => return Err(MALFORMED),
            _ => {} // Optional unknown chunks inside ANMF can also carry metadata.
        }
    }
    if !image {
        return Err(MALFORMED);
    }
    Ok(output)
}

pub(super) fn webp(data: &[u8]) -> CleanResult {
    let header = take(data, 0, 12)?;
    if &header[..4] != b"RIFF"
        || &header[8..] != b"WEBP"
        || le32(&header[4..]) as u64 + 8 != data.len() as u64
    {
        return Err(MALFORMED);
    }
    let mut output = header.to_vec();
    let mut offset = 12;
    let mut extended = None;
    let mut profile = false;
    let mut animation = false;
    let mut alpha = false;
    let mut images = 0;
    while offset < data.len() {
        let start = offset;
        let (kind, payload) = riff_chunk(data, &mut offset)?;
        match kind {
            b"VP8X" => {
                if start != 12 || payload.len() != 10 {
                    return Err(MALFORMED);
                }
                let width = le24(&payload[4..]) + 1;
                let height = le24(&payload[7..]) + 1;
                if width as u64 * height as u64 > u32::MAX as u64 {
                    return Err(MALFORMED);
                }
                extended = Some((payload[0], width, height));
                let mut clean = payload.to_vec();
                clean[0] &= !0x0c;
                push_chunk(&mut output, kind, &clean);
            }
            b"ICCP" => {
                if profile
                    || animation
                    || alpha
                    || images > 0
                    || extended.is_none_or(|h| h.0 & 0x20 == 0)
                    || payload.is_empty()
                {
                    return Err(MALFORMED);
                }
                profile = true;
                push_chunk(&mut output, kind, payload);
            }
            b"ANIM" => {
                if animation
                    || alpha
                    || images > 0
                    || payload.len() != 6
                    || extended.is_none_or(|h| h.0 & 2 == 0)
                {
                    return Err(MALFORMED);
                }
                animation = true;
                push_chunk(&mut output, kind, payload);
            }
            b"ANMF" => {
                if !animation {
                    return Err(MALFORMED);
                }
                let frame = take(payload, 0, 16)?;
                let width = le24(&frame[6..]) + 1;
                let height = le24(&frame[9..]) + 1;
                let (_, canvas_w, canvas_h) = extended.ok_or(MALFORMED)?;
                if le24(frame) * 2 + width > canvas_w || le24(&frame[3..]) * 2 + height > canvas_h {
                    return Err(MALFORMED);
                }
                let mut clean = frame.to_vec();
                clean.extend_from_slice(&webp_frame(&payload[16..], (width, height))?);
                push_chunk(&mut output, kind, &clean);
                images += 1;
            }
            b"ALPH" => {
                if alpha || animation || images > 0 || extended.is_none() || payload.is_empty() {
                    return Err(MALFORMED);
                }
                alpha = true;
                push_chunk(&mut output, kind, payload);
            }
            b"VP8 " | b"VP8L" => {
                if animation || images > 0 || (kind == b"VP8L" && alpha) {
                    return Err(MALFORMED);
                }
                let dimensions = image_dimensions(kind, payload)?;
                if extended.is_some_and(|(_, w, h)| (w, h) != dimensions) {
                    return Err(MALFORMED);
                }
                images += 1;
                push_chunk(&mut output, kind, payload);
            }
            _ => {} // EXIF, XMP and all private/unknown ancillary payloads.
        }
    }
    if images == 0
        || extended.is_some_and(|(flags, _, _)| {
            (flags & 2 != 0) != animation || (flags & 0x20 != 0) != profile
        })
    {
        return Err(MALFORMED);
    }
    let size = u32::try_from(output.len() - 8).map_err(|_| MALFORMED)?;
    output[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PRIVATE: &[u8] = b"ANNEX_PRIVATE_METADATA_LOCATION_AUTHOR_12345";
    const FIXTURES: [(&str, &[u8]); 5] = [
        (
            "progressive.jpg",
            include_bytes!("../../tests/fixtures/image-privacy/progressive.jpg"),
        ),
        (
            "animated.png",
            include_bytes!("../../tests/fixtures/image-privacy/animated.png"),
        ),
        (
            "animated.gif",
            include_bytes!("../../tests/fixtures/image-privacy/animated.gif"),
        ),
        (
            "animated.webp",
            include_bytes!("../../tests/fixtures/image-privacy/animated.webp"),
        ),
        (
            "still.webp",
            include_bytes!("../../tests/fixtures/image-privacy/still.webp"),
        ),
    ];
    fn clean(name: &str, data: &[u8]) -> CleanResult {
        match name.rsplit('.').next().unwrap() {
            "jpg" => jpeg(data),
            "png" => png(data),
            "gif" => gif(data),
            _ => webp(data),
        }
    }
    #[test]
    fn real_encoded_images_strip_private_metadata_and_remain_idempotent() {
        for (name, data) in FIXTURES {
            assert!(data.windows(PRIVATE.len()).any(|p| p == PRIVATE));
            let output = clean(name, data).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                !output.windows(PRIVATE.len()).any(|p| p == PRIVATE),
                "{name}"
            );
            assert_eq!(clean(name, &output).unwrap(), output, "{name}");
            assert!(output.len() < data.len(), "{name}");
        }
    }
    #[test]
    fn every_truncated_fixture_is_refused_instead_of_partially_copied() {
        for (name, data) in FIXTURES {
            for length in 0..data.len() {
                assert!(
                    clean(name, &data[..length]).is_err(),
                    "{name} truncation at {length}"
                );
            }
        }
    }
    #[test]
    fn forged_container_lengths_and_crc_are_refused() {
        let mut png_bytes = FIXTURES[1].1.to_vec();
        png_bytes[29] ^= 1;
        assert!(png(&png_bytes).is_err());
        let mut webp_bytes = FIXTURES[3].1.to_vec();
        webp_bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(webp(&webp_bytes).is_err());
        let mut jpeg_bytes = FIXTURES[0].1.to_vec();
        jpeg_bytes[4..6].copy_from_slice(&[0, 1]);
        assert!(jpeg(&jpeg_bytes).is_err());
    }
    #[test]
    fn loop_extension_cannot_smuggle_an_extra_metadata_block() {
        let source = FIXTURES[2].1;
        let label = source
            .windows(11)
            .position(|p| p == b"NETSCAPE2.0")
            .unwrap();
        let terminator = label + 11 + 4;
        assert_eq!(source[terminator], 0);
        let mut altered = source[..terminator].to_vec();
        altered.push(PRIVATE.len() as u8);
        altered.extend_from_slice(PRIVATE);
        altered.extend_from_slice(&source[terminator..]);
        assert!(gif(&altered).is_err());
    }
    #[test]
    fn jpeg_interscan_comments_are_removed_from_a_real_progressive_image() {
        let source = FIXTURES[0].1;
        let scan = source.windows(2).position(|p| p == [0xff, 0xda]).unwrap();
        let second = source[scan + 2..]
            .windows(2)
            .position(|p| p == [0xff, 0xda])
            .unwrap()
            + scan
            + 2;
        let mut altered = source[..second].to_vec();
        altered.extend_from_slice(&[0xff, 0xfe]);
        altered.extend_from_slice(&((PRIVATE.len() + 2) as u16).to_be_bytes());
        altered.extend_from_slice(PRIVATE);
        altered.extend_from_slice(&source[second..]);
        assert_eq!(jpeg(&altered).unwrap(), jpeg(source).unwrap());
    }
}
