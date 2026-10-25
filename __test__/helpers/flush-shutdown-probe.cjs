'use strict'

const { spawnSync } = require('node:child_process')
const { readFileSync } = require('node:fs')
const { Worker } = require('node:worker_threads')

const addon = require.resolve('../../index.js')
// Keep the native runtime registered in the main environment while workers end.
require(addon)

function nativeThreads() {
  if (process.platform === 'linux') {
    const lines = readFileSync('/proc/self/status', 'utf8').split('\n')
    const entry = lines.find((line) => line.startsWith('Threads:'))
    if (!entry) throw new Error('Threads entry missing from /proc/self/status')
    return Number(entry.slice('Threads:'.length).trim())
  }
  if (process.platform === 'darwin') {
    const result = spawnSync('/bin/ps', ['-M', '-p', String(process.pid)], { encoding: 'utf8' })
    if (result.status !== 0) throw new Error(result.stderr || `ps exited ${result.status}`)
    return result.stdout.trim().split('\n').length - 1
  }
  if (process.platform === 'win32') {
    const command = `(Get-Process -Id ${process.pid}).Threads.Count`
    const result = spawnSync('powershell.exe', ['-NoProfile', '-Command', command], { encoding: 'utf8' })
    if (result.status !== 0) throw new Error(result.stderr || `powershell exited ${result.status}`)
    return Number(result.stdout.trim())
  }
  throw new Error(`Unsupported platform: ${process.platform}`)
}

const workerSource = String.raw`
'use strict'
const { parentPort, workerData } = require('node:worker_threads')
const { AudioData, AudioEncoder } = require(workerData.addon)
const sleeper = new Int32Array(new SharedArrayBuffer(4))
const pause = (ms) => Atomics.wait(sleeper, 0, 0, ms)
const config = { codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2, bitrate: 128000 }
const input = (i) => new AudioData({
  data: new Uint8Array(1024 * 2 * 4),
  format: 'f32', sampleRate: 48000, numberOfFrames: 1024, numberOfChannels: 2,
  timestamp: Math.floor(i * 1024 * 1000000 / 48000),
})
const encode = (i) => {
  const data = input(i)
  codec.encode(data)
  data.close()
}
let outputs = 0
let armed = false
let checkpoint = false
const codec = new AudioEncoder({
  output() {
    outputs++
    const queueSize = codec.encodeQueueSize
    if (armed && !checkpoint && outputs >= 1000 && queueSize <= 5000) {
      checkpoint = true
      queueMicrotask(() => {
        parentPort.postMessage({ type: 'checkpoint', outputs, queueSize })
        pause(10000)
      })
    }
  },
  error() {},
})
codec.configure(config)
for (let i = 0; i < 2501; i++) encode(i)
Promise.resolve().then(() => Promise.resolve()).then(async () => {
  const deadline = Date.now() + 15000
  while (codec.encodeQueueSize > 0 && Date.now() < deadline) pause(1)
  pause(100)
  const flushing = codec.flush()
  const observed = Promise.allSettled([flushing])
  await Promise.resolve()
  await Promise.resolve()
  pause(1000)
  for (let i = 2501; i < 32501; i++) encode(i)
  codec.configure({ ...config, sampleRate: 768000 })
  await Promise.resolve()
  await Promise.resolve()
  armed = true
  parentPort.postMessage({ type: 'armed', queueSize: codec.encodeQueueSize })
  await observed
}).catch((error) => {
  parentPort.postMessage({ type: 'worker-error', error: String(error && error.stack || error) })
})
`

async function runOne(iteration) {
  const before = nativeThreads()
  const worker = new Worker(workerSource, { eval: true, workerData: { addon } })
  let timer
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`iteration ${iteration} timed out`)), 60_000)
  })
  const observation = (async () => {
    worker.on('message', (message) => {
      if (message.type === 'checkpoint') {
        void worker.terminate()
      } else if (message.type === 'worker-error') {
        console.error(message.error)
        void worker.terminate()
      }
    })
    return new Promise((resolve, reject) => {
      worker.once('exit', resolve)
      worker.once('error', reject)
    })
  })()
  const exitCode = await Promise.race([observation, timeout]).finally(() => {
    clearTimeout(timer)
    void worker.terminate()
  })
  await new Promise((resolve) => setTimeout(resolve, 1000))
  const after = nativeThreads()
  return { iteration, before, after, delta: after - before, exitCode }
}

async function main() {
  const summary = { initialThreads: nativeThreads(), finalThreads: 0, rows: [] }
  for (let iteration = 1; iteration <= 6; iteration++) {
    summary.rows.push(await runOne(iteration))
  }
  summary.finalThreads = nativeThreads()
  console.log('FLUSH_SHUTDOWN_SUMMARY ' + JSON.stringify(summary))
}

main().then(
  () => process.exit(0),
  (error) => {
    console.error(error)
    process.exit(2)
  },
)
