/** Remove image metadata without decoding or flattening animation frames. */
import { E2eError } from './e2e';

const ascii = (bytes: Uint8Array, offset: number, length: number) => String.fromCharCode(...bytes.subarray(offset, offset + length));
function malformed(): never { throw new E2eError('The image container is malformed. It was not uploaded.'); }
function join(chunks: Uint8Array[]): Uint8Array {
  const output = new Uint8Array(chunks.reduce((sum, item) => sum + item.length, 0));
  let offset = 0;
  for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.length; }
  return output;
}
function need(bytes: Uint8Array, offset: number, count: number): void {
  if (!Number.isSafeInteger(offset + count) || offset < 0 || count < 0 || offset + count > bytes.length) malformed();
}
function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function jpeg(bytes: Uint8Array): Uint8Array {
  if (bytes.length < 4 || bytes[0] !== 0xff || bytes[1] !== 0xd8) return malformed();
  const result = [bytes.subarray(0, 2)];
  let offset = 2;
  let image = false;
  while (offset < bytes.length) {
    const start = offset;
    if (bytes[offset++] !== 0xff) return malformed();
    while (bytes[offset] === 0xff) offset++;
    need(bytes, offset, 1);
    const marker = bytes[offset++];
    if (marker === 0xd9) { if (!image) return malformed(); result.push(bytes.subarray(start, offset)); return join(result); }
    if (marker === 0x00 || marker === 0xd8) return malformed();
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) { result.push(bytes.subarray(start, offset)); continue; }
    need(bytes, offset, 2);
    const length = (bytes[offset] << 8) | bytes[offset + 1];
    if (length < 2) return malformed();
    need(bytes, offset, length);
    const end = offset + length;
    // JFIF and Adobe color interpretation remain. Remove comments and other
    // application segments, including EXIF/GPS, XMP, IPTC and thumbnails.
    const colorProfile = marker === 0xe2 && length >= 16 && ascii(bytes, offset + 2, 12) === 'ICC_PROFILE\0';
    const adobe = marker === 0xee && length === 14 && ascii(bytes, offset + 2, 5) === 'Adobe';
    if (marker === 0xe0 && length >= 16 && ascii(bytes, offset + 2, 5) === 'JFIF\0') {
      // Embedded thumbnails can contain pixels or data unrelated to the image.
      const jfif = bytes.slice(start, offset + 16);
      const lengthOffset = offset - start;
      jfif[lengthOffset] = 0; jfif[lengthOffset + 1] = 16;
      jfif[lengthOffset + 14] = 0; jfif[lengthOffset + 15] = 0;
      result.push(jfif);
    } else if (colorProfile || adobe || !(marker === 0xfe || (marker >= 0xe0 && marker <= 0xef))) result.push(bytes.subarray(start, end));
    offset = end;
    if (marker === 0xda) {
      image = true;
      const entropyStart = offset;
      while (offset < bytes.length) {
        if (bytes[offset] !== 0xff) { offset++; continue; }
        need(bytes, offset, 2);
        const next = bytes[offset + 1];
        if (next === 0 || (next >= 0xd0 && next <= 0xd7)) { offset += 2; continue; }
        break;
      }
      result.push(bytes.subarray(entropyStart, offset));
    }
  }
  return malformed();
}

function png(bytes: Uint8Array): Uint8Array {
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (!signature.every((value, i) => bytes[i] === value)) return malformed();
  const chunks = [bytes.subarray(0, 8)];
  // Preserve reconstruction/color/animation chunks, omit text, EXIF, timestamps
  // and private ancillary chunks. Compressed pixel and APNG frame bytes survive.
  const retained = new Set(['IHDR', 'PLTE', 'IDAT', 'IEND', 'tRNS', 'gAMA', 'cHRM', 'sRGB', 'iCCP', 'sBIT', 'bKGD', 'pHYs', 'acTL', 'fcTL', 'fdAT', 'cICP', 'mDCv', 'cLLi']);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = 8;
  let first = true;
  let image = false;
  while (offset < bytes.length) {
    need(bytes, offset, 12);
    const length = view.getUint32(offset);
    const type = ascii(bytes, offset + 4, 4);
    need(bytes, offset, 12 + length);
    if (crc32(bytes.subarray(offset + 4, offset + 8 + length)) !== view.getUint32(offset + 8 + length)) return malformed();
    if (first && (type !== 'IHDR' || length !== 13)) return malformed();
    if (!/^[A-Za-z]{4}$/.test(type) || (!first && type === 'IHDR')) return malformed();
    first = false;
    if (type === 'IDAT' && length > 0) image = true;
    if (retained.has(type)) chunks.push(bytes.subarray(offset, offset + length + 12));
    else if ((bytes[offset + 4] & 32) === 0) return malformed();
    offset += length + 12;
    if (type === 'IEND') {
      if (length !== 0 || !image) return malformed();
      return join(chunks);
    }
  }
  return malformed();
}

function gif(bytes: Uint8Array): Uint8Array {
  need(bytes, 0, 13);
  if (!['GIF87a', 'GIF89a'].includes(ascii(bytes, 0, 6))) return malformed();
  let offset = 13 + ((bytes[10] & 0x80) ? 3 * 2 ** ((bytes[10] & 7) + 1) : 0);
  need(bytes, 0, offset);
  const chunks = [bytes.subarray(0, offset)];
  let image = false;
  const blocks = () => {
    while (true) {
      need(bytes, offset, 1);
      const size = bytes[offset++];
      if (size === 0) return;
      need(bytes, offset, size); offset += size;
    }
  };
  while (offset < bytes.length) {
    const start = offset;
    const introducer = bytes[offset++];
    if (introducer === 0x3b) { if (!image) return malformed(); chunks.push(bytes.subarray(start, offset)); return join(chunks); }
    if (introducer === 0x2c) {
      need(bytes, offset, 9);
      const packed = bytes[offset + 8]; offset += 9;
      if (packed & 0x80) offset += 3 * 2 ** ((packed & 7) + 1);
      need(bytes, offset, 2);
      if (bytes[offset] < 2 || bytes[offset] > 8 || bytes[offset + 1] === 0) return malformed();
      offset++; blocks(); image = true;
      chunks.push(bytes.subarray(start, offset));
    } else if (introducer === 0x21) {
      need(bytes, offset, 2);
      const label = bytes[offset++];
      const app = label === 0xff && bytes[offset] === 11 ? ascii(bytes, offset + 1, 11) : '';
      const extensionStart = offset;
      blocks();
      if (label === 0xf9 && (offset - extensionStart !== 6 || bytes[extensionStart] !== 4)) return malformed();
      if (label === 0x01 && bytes[extensionStart] !== 12) return malformed();
      if ((app === 'NETSCAPE2.0' || app === 'ANIMEXTS1.0') &&
        (offset - extensionStart !== 17 || bytes[extensionStart + 12] !== 3 || bytes[extensionStart + 13] !== 1)) return malformed();
      // Graphic control, rendered text, and loop controls affect presentation.
      // Comments, XMP, and unknown application extensions carry metadata.
      if (label === 0xf9 || label === 0x01 || app === 'NETSCAPE2.0' || app === 'ANIMEXTS1.0') chunks.push(bytes.subarray(start, offset));
    } else return malformed();
  }
  return malformed();
}

function webp(bytes: Uint8Array): Uint8Array {
  need(bytes, 0, 12);
  if (ascii(bytes, 0, 4) !== 'RIFF' || ascii(bytes, 8, 4) !== 'WEBP') return malformed();
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const declared = view.getUint32(4, true) + 8;
  if (declared !== bytes.length) return malformed();
  const clean = (start: number, end: number, nested: boolean): Uint8Array => {
    const chunks: Uint8Array[] = [];
    let image = false;
    for (let offset = start; offset < end;) {
      if (offset + 8 > end) return malformed();
      const type = ascii(bytes, offset, 4);
      const length = view.getUint32(offset + 4, true);
      const padded = 8 + length + (length & 1);
      if (offset + padded > end) return malformed();
      let chunk: Uint8Array = bytes.slice(offset, offset + padded);
      if (type === 'VP8 ' || type === 'VP8L') {
        if (length === 0) return malformed();
        image = true; chunks.push(chunk);
      } else if (type === 'ALPH') {
        if (length === 0) return malformed();
        chunks.push(chunk);
      } else if (!nested && type === 'ANMF') {
        if (length < 16) return malformed();
        const payload = clean(offset + 24, offset + 8 + length, true);
        chunk = join([chunk.subarray(0, 24), payload]);
        new DataView(chunk.buffer).setUint32(4, chunk.length - 8, true);
        image = true; chunks.push(chunk);
      } else if (!nested && ['VP8X', 'ICCP', 'ANIM'].includes(type)) {
        if (type === 'VP8X') {
          if (length !== 10) return malformed();
          chunk[8] &= ~0x0c;
        } else if (type === 'ANIM' && length !== 6) return malformed();
        chunks.push(chunk);
      }
      offset += padded;
    }
    if (!image) return malformed();
    return join(chunks);
  };
  const output = join([bytes.slice(0, 12), clean(12, bytes.length, false)]);
  new DataView(output.buffer).setUint32(4, output.length - 8, true);
  return output;
}

export function stripImageMetadata(bytes: Uint8Array, contentType: string): Uint8Array {
  switch (contentType) {
    case 'image/jpeg': return jpeg(bytes);
    case 'image/png': return png(bytes);
    case 'image/gif': return gif(bytes);
    case 'image/webp': return webp(bytes);
    default: throw new E2eError('Unsupported image format');
  }
}
