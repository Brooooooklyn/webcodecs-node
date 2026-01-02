// import { VideoEncoder, VideoFrame, Mp4Muxer, createCanvas } from './index.js';
import fs from 'fs';

import { createCanvas } from '@napi-rs/canvas';
import pkg from './index.js';
const { VideoEncoder, VideoFrame, Mp4Muxer } = pkg;

const width = 400, height = 400, fps = 30, duration = 1;
const canvas = createCanvas(width, height);
const ctx = canvas.getContext('2d');

const chunks = [];
const encoder = new VideoEncoder({
  output: (chunk, metadata) => {
    chunks.push({ chunk, metadata });
  },
  error: (e) => console.error('Encoder error:', e),
});

encoder.configure({
  codec: 'hvc1.1.6.L93.B0',
  width, height,
  bitrate: 1_000_000,
  framerate: fps,
});

// Encode frames
for (let i = 0; i < fps * duration; i++) {
  ctx.fillStyle = `hsl(${(i * 12) % 360}, 70%, 50%)`;
  ctx.fillRect(0, 0, width, height);
  ctx.fillStyle = 'white';
  ctx.font = '48px sans-serif';
  ctx.fillText(`Frame ${i}`, 50, 100);
  
  const frame = new VideoFrame(canvas, { timestamp: i * (1_000_000 / fps) });
  encoder.encode(frame, { keyFrame: i === 0 });
  frame.close();
}

await encoder.flush();

console.log(`Collected ${chunks.length} chunks`);
console.log('\nChunk timestamps (microseconds):');
chunks.forEach((c, i) => {
  console.log(`  Chunk ${i}: timestamp=${c.chunk.timestamp}, type=${c.chunk.type}`);
});

