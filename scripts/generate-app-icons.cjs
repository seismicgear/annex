// Package the owner's approved PNG without redrawing, cropping, recoloring, or
// removing its background. Uses Node built-ins only; originals remain unchanged.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

function decodePng(bytes) {
  const signature = Buffer.from([137,80,78,71,13,10,26,10]);
  if (!bytes.subarray(0, 8).equals(signature)) throw new Error('Source must be a PNG');
  let width, height, channels;
  const parts = [];
  let ended = false;
  for (let pos = 8; pos + 12 <= bytes.length;) {
    const length = bytes.readUInt32BE(pos);
    const end = pos + 12 + length;
    if (end > bytes.length) throw new Error('Truncated PNG chunk');
    const type = bytes.toString('ascii', pos + 4, pos + 8);
    const data = bytes.subarray(pos + 8, pos + 8 + length);
    if (crc32(bytes.subarray(pos + 4, pos + 8 + length)) !== bytes.readUInt32BE(pos + 8 + length)) throw new Error('Invalid PNG checksum');
    if (type === 'IHDR') {
      if (length !== 13 || data[8] !== 8 || ![2,6].includes(data[9]) || data[10] !== 0 || data[11] !== 0 || data[12] !== 0) throw new Error('Expected non-interlaced 8-bit RGB/RGBA PNG');
      width = data.readUInt32BE(0); height = data.readUInt32BE(4);
      channels = data[9] === 6 ? 4 : 3;
      if (width !== height || width < 1024 || width > 8192) throw new Error('Approved source must be square and at least 1024px');
    } else if (type === 'IDAT') parts.push(data);
    else if (type === 'IEND') { ended = true; break; }
    else if (type === 'tRNS') throw new Error('Use an RGBA source for transparency');
    pos = end;
  }
  if (!ended || !width || !parts.length) throw new Error('Incomplete PNG');
  const stride = width * channels;
  const expected = (stride + 1) * height;
  const raw = zlib.inflateSync(Buffer.concat(parts), { maxOutputLength: expected });
  if (raw.length !== expected) throw new Error('Unexpected PNG scanline length');
  const pixels = Buffer.alloc(stride * height);
  const paeth = (a,b,c) => { const p=a+b-c, pa=Math.abs(p-a), pb=Math.abs(p-b), pc=Math.abs(p-c); return pa<=pb && pa<=pc ? a : pb<=pc ? b : c; };
  for (let y=0; y<height; y++) {
    const filter=raw[y*(stride+1)];
    if (filter>4) throw new Error('Invalid PNG filter');
    for (let x=0; x<stride; x++) {
      const i=y*stride+x, left=x>=channels ? pixels[i-channels] : 0, up=y ? pixels[i-stride] : 0, corner=y && x>=channels ? pixels[i-stride-channels] : 0;
      const predictor=[0,left,up,Math.floor((left+up)/2),paeth(left,up,corner)][filter];
      pixels[i]=(raw[y*(stride+1)+1+x]+predictor)&255;
    }
  }
  const rgba=Buffer.alloc(width*height*4);
  for(let i=0; i<width*height; i++) {
    rgba[i*4]=pixels[i*channels]; rgba[i*4+1]=pixels[i*channels+1]; rgba[i*4+2]=pixels[i*channels+2]; rgba[i*4+3]=channels===4 ? pixels[i*channels+3] : 255;
  }
  return { data: rgba, size: width };
}

// Area resampling retains the whole square and the original black background.
function downscale(master, sourceSize, target) {
  const out=new Uint8ClampedArray(target*target*4), scale=sourceSize/target;
  for(let oy=0; oy<target; oy++) for(let ox=0; ox<target; ox++) {
    const x0=ox*scale, x1=(ox+1)*scale, y0=oy*scale, y1=(oy+1)*scale;
    let red=0, green=0, blue=0, alpha=0, weight=0;
    for(let sy=Math.floor(y0); sy<Math.ceil(y1); sy++) for(let sx=Math.floor(x0); sx<Math.ceil(x1); sx++) {
      if(sx>=sourceSize || sy>=sourceSize) continue;
      const w=(Math.min(x1,sx+1)-Math.max(x0,sx))*(Math.min(y1,sy+1)-Math.max(y0,sy));
      const i=(sy*sourceSize+sx)*4, a=master[i+3]/255;
      red+=master[i]*a*w; green+=master[i+1]*a*w; blue+=master[i+2]*a*w; alpha+=a*w; weight+=w;
    }
    const i=(oy*target+ox)*4;
    out[i]=alpha ? red/alpha : 0; out[i+1]=alpha ? green/alpha : 0; out[i+2]=alpha ? blue/alpha : 0; out[i+3]=255*alpha/weight;
  }
  return out;
}

function crc32(buf) {
  if (typeof zlib.crc32 === 'function') return zlib.crc32(buf) >>> 0;
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
  }
  return ~c >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, 'ascii');
  const body = Buffer.concat([typeBuf, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([len, body, crc]);
}
function encodePng(rgba, size) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;
  // raw scanlines with filter byte 0
  const stride = size * 4;
  const raw = Buffer.alloc((stride + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0;
    Buffer.from(rgba.buffer, rgba.byteOffset + y * stride, stride).copy(
      raw,
      y * (stride + 1) + 1,
    );
  }
  const idat = zlib.deflateSync(raw, { level: 9 });
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', idat), chunk('IEND', Buffer.alloc(0))]);
}

// ── ICO encoder (PNG-encoded entries) ───────────────────────────────────
function encodeIco(pngsBySize) {
  const sizes = Object.keys(pngsBySize)
    .map(Number)
    .sort((a, b) => a - b);
  const count = sizes.length;
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2); // ICO
  header.writeUInt16LE(count, 4);
  const dir = Buffer.alloc(16 * count);
  let offset = 6 + 16 * count;
  const blobs = [];
  sizes.forEach((sz, idx) => {
    const png = pngsBySize[sz];
    const e = idx * 16;
    dir[e] = sz >= 256 ? 0 : sz;
    dir[e + 1] = sz >= 256 ? 0 : sz;
    dir[e + 2] = 0;
    dir[e + 3] = 0;
    dir.writeUInt16LE(1, e + 4); // planes
    dir.writeUInt16LE(32, e + 6); // bpp
    dir.writeUInt32LE(png.length, e + 8);
    dir.writeUInt32LE(offset, e + 12);
    offset += png.length;
    blobs.push(png);
  });
  return Buffer.concat([header, dir, ...blobs]);
}

// ── ICNS encoder (PNG-encoded entries) ──────────────────────────────────
function encodeIcns(pngsBySize) {
  // ostype per size; @1x and @2x variants share the PNG.
  const map = {
    16: ['icp4'],
    32: ['icp5', 'ic11'],
    64: ['icp6', 'ic12'],
    128: ['ic07'],
    256: ['ic08', 'ic13'],
    512: ['ic09', 'ic14'],
    1024: ['ic10'],
  };
  const entries = [];
  for (const [szStr, types] of Object.entries(map)) {
    const sz = Number(szStr);
    const png = pngsBySize[sz];
    if (!png) continue;
    for (const t of types) {
      const len = Buffer.alloc(4);
      len.writeUInt32BE(png.length + 8, 0);
      entries.push(Buffer.concat([Buffer.from(t, 'ascii'), len, png]));
    }
  }
  const body = Buffer.concat(entries);
  const total = Buffer.alloc(8);
  total.write('icns', 0, 'ascii');
  total.writeUInt32BE(body.length + 8, 4);
  return Buffer.concat([total, body]);
}

function generate(source, destination, pngSizes = []) {
  if (!source || !destination) throw new Error('Usage: node generate-app-icons.cjs <approved.png> <output-directory>');
  const {data,size}=decodePng(fs.readFileSync(source));
  fs.mkdirSync(destination,{recursive:true});
  const sizes=[16,32,48,64,128,256,512,1024];
  const pngs={};
  for(const n of sizes) pngs[n]=encodePng(downscale(data,size,n),n);
  for(const n of pngSizes) {
    if (!pngs[n]) throw new Error(`Unsupported icon size ${n}`);
    fs.writeFileSync(path.join(destination,`${n}x${n}.png`),pngs[n]);
  }
  fs.writeFileSync(path.join(destination,'icon.png'),pngs[1024]);
  fs.writeFileSync(path.join(destination,'icon.ico'),encodeIco(Object.fromEntries(sizes.filter(n=>n<=256).map(n=>[n,pngs[n]]))));
  fs.writeFileSync(path.join(destination,'icon.icns'),encodeIcns(pngs));
  process.stdout.write(`Packaged approved artwork from ${source} into ${destination}\n`);
}
module.exports = { generate, decodePng };
if (require.main === module) generate(process.argv[2],process.argv[3],(process.argv[4] || "").split(",").filter(Boolean).map(Number));
