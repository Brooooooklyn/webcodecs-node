'use strict'

const { AudioData, AudioDecoder, AudioEncoder, EncodedAudioChunk } = require('../../index.js')

const ITERATIONS = Number(process.env.FLUSH_ATOMICITY_ITERATIONS || 2000)
const SUMMARY_MARKER = 'FLUSH_ATOMICITY_SUMMARY '
const FLUSH_TIMEOUT_MS = 5000
const ERROR_TIMEOUT_MS = 5000
const AUDIO_CONFIG = {
  codec: 'mp4a.40.2',
  sampleRate: 48000,
  numberOfChannels: 2,
  bitrate: 128000,
}

function waitWithTimeout(promise, timeoutMs, message) {
  let timer
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(message)), timeoutMs)
    }),
  ]).finally(() => clearTimeout(timer))
}

function audioInput(index) {
  return new AudioData({
    data: new Uint8Array(1024 * 2 * 4),
    format: 'f32',
    sampleRate: 48000,
    numberOfFrames: 1024,
    numberOfChannels: 2,
    timestamp: Math.floor((index * 1024 * 1_000_000) / 48000),
  })
}

async function encodedInput() {
  const chunks = []
  let decoderConfig
  let codecError
  const encoder = new AudioEncoder({
    output(chunk, metadata) {
      const data = new Uint8Array(chunk.byteLength)
      chunk.copyTo(data)
      chunks.push(
        new EncodedAudioChunk({
          type: chunk.type,
          timestamp: chunk.timestamp,
          duration: chunk.duration,
          data,
        }),
      )
      decoderConfig ||= metadata?.decoderConfig
    },
    error(error) {
      codecError = error
    },
  })
  encoder.configure(AUDIO_CONFIG)
  // Four AAC input frames produce five packets, including encoder delay.
  for (let i = 0; i < 4; i++) {
    const data = audioInput(i)
    encoder.encode(data)
    data.close()
  }
  await encoder.flush()
  encoder.close()
  if (codecError || chunks.length !== 5 || !decoderConfig) {
    throw new Error(`input generation failed: chunks=${chunks.length} error=${codecError}`)
  }
  return { chunks, decoderConfig }
}

async function expectedOutputs(chunks, decoderConfig) {
  let outputs = 0
  let codecError
  const decoder = new AudioDecoder({
    output(data) {
      outputs++
      data.close()
    },
    error(error) {
      codecError = error
    },
  })
  decoder.configure(decoderConfig)
  for (const chunk of chunks) decoder.decode(chunk)
  await decoder.flush()
  await new Promise((resolve) => setImmediate(resolve))
  decoder.close()
  if (codecError) throw codecError
  return outputs
}

async function runOnce(iteration, chunks, decoderConfig, expected) {
  let outputs = 0
  let errors = 0
  let outputsAtError = -1
  let outputsAfterError = 0
  let atSettle = -1
  let settlement = 'pending'
  let notifyError
  // Resolve from error() so we wait for the callback itself, not a sleep that
  // close() can outrun. Flush does not wait for this configure-error tail:
  // send_error is a later ThreadsafeFunction wakeup, and close() advances the
  // epoch which drops an undelivered Work error (Chrome-compatible).
  const errorSeen = new Promise((resolve) => {
    notifyError = resolve
  })
  const decoder = new AudioDecoder({
    output(data) {
      if (errors > 0) outputsAfterError++
      outputs++
      data.close()
    },
    error() {
      errors++
      outputsAtError = outputs
      notifyError()
    },
  })
  decoder.configure(decoderConfig)
  for (const chunk of chunks) decoder.decode(chunk)

  const flushing = decoder.flush()
  // The worker answers Flush first, then fails to open this replacement decoder.
  decoder.configure({ ...decoderConfig, sampleRate: 768000 })
  const observed = flushing.then(
    () => {
      atSettle = outputs
      settlement = 'resolved'
    },
    (error) => {
      atSettle = outputs
      settlement = `rejected:${error.name}`
    },
  )

  try {
    await waitWithTimeout(observed, FLUSH_TIMEOUT_MS, `flush timeout at iteration ${iteration}`)
    await waitWithTimeout(
      errorSeen,
      ERROR_TIMEOUT_MS,
      `error callback timeout at iteration ${iteration}`,
    )
  } finally {
    try {
      decoder.close()
    } catch {}
  }

  return {
    iteration,
    settlement,
    atSettle,
    finalOutputs: outputs,
    errors,
    ordered: errors === 1 && outputsAtError === expected && outputsAfterError === 0,
  }
}

async function main() {
  const started = Date.now()
  const { chunks, decoderConfig } = await encodedInput()
  const expected = await expectedOutputs(chunks, decoderConfig)
  if (expected !== 5) throw new Error(`baseline produced ${expected} outputs instead of 5`)

  const summary = {
    iterations: ITERATIONS,
    completed: 0,
    expected,
    races: 0,
    early: 0,
    incomplete: 0,
    badErrorCounts: 0,
    badOrdering: 0,
    timeouts: 0,
    firstFailures: [],
    elapsedMs: 0,
  }

  for (let iteration = 1; iteration <= ITERATIONS; iteration++) {
    let result
    try {
      result = await runOnce(iteration, chunks, decoderConfig, expected)
    } catch (error) {
      summary.timeouts++
      summary.firstFailures.push({ iteration, error: String(error) })
      break
    }
    summary.completed++
    if (result.atSettle < expected && result.finalOutputs > result.atSettle) summary.races++
    if (result.atSettle !== expected) summary.early++
    if (result.finalOutputs !== expected) summary.incomplete++
    if (result.errors !== 1) summary.badErrorCounts++
    if (!result.ordered) summary.badOrdering++
    if (
      result.atSettle !== expected ||
      result.finalOutputs !== expected ||
      result.errors !== 1 ||
      !result.ordered
    ) {
      if (summary.firstFailures.length < 5) summary.firstFailures.push(result)
    }
  }

  summary.elapsedMs = Date.now() - started
  console.log(SUMMARY_MARKER + JSON.stringify(summary))
  if (
    summary.completed !== ITERATIONS ||
    summary.early ||
    summary.incomplete ||
    summary.badErrorCounts ||
    summary.badOrdering ||
    summary.timeouts
  ) {
    process.exitCode = 1
  }
}

main().catch((error) => {
  console.error(error)
  process.exitCode = 2
})
