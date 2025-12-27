/**
 * Performance Comparison: @napi-rs/webcodecs vs webcodecs-polyfill
 *
 * Uses real video file (small_buck_bunny.mp4) for realistic benchmarking.
 * Runs each implementation in a separate process to avoid FFmpeg global state conflicts.
 *
 * Run with: node --import @oxc-node/core/register benchmark/compare.ts
 */

import { execSync } from 'node:child_process'

import chalk from 'chalk'

interface Results {
  decodeFps: number
  encodeFps: number
  encodeBytesPerFrame: number
  totalFrames: number
  resolution: string
}

function speedup(a: number, b: number): string {
  const ratio = a / b
  if (ratio > 1.05) {
    return chalk.green(`${ratio.toFixed(2)}x faster`)
  } else if (ratio < 0.95) {
    return chalk.red(`${(1 / ratio).toFixed(2)}x slower`)
  }
  return chalk.yellow('~same')
}

console.log(chalk.bold.white('\n' + '='.repeat(70)))
console.log(chalk.bold.white(' Performance: @napi-rs/webcodecs vs webcodecs-polyfill'))
console.log(chalk.bold.white('='.repeat(70)))
console.log()
console.log(chalk.gray(`  Node.js: ${process.version}`))
console.log(chalk.gray(`  Platform: ${process.platform} ${process.arch}`))
console.log(chalk.gray(`  Video: small_buck_bunny.mp4`))

// Run napi-rs benchmark
console.log(chalk.cyan('\nRunning @napi-rs/webcodecs benchmark...'))
const napiStart = performance.now()
const napiOutput = execSync('node benchmark/bench-napi.ts', {
  encoding: 'utf-8',
  stdio: ['pipe', 'pipe', 'pipe'],
  cwd: process.cwd(),
})
const napiTime = performance.now() - napiStart
const napiResults: Results = JSON.parse(napiOutput.trim())
console.log(chalk.green(`  Done in ${(napiTime / 1000).toFixed(2)}s`))

// Run polyfill benchmark
console.log(chalk.cyan('Running webcodecs-polyfill benchmark...'))
const polyStart = performance.now()
const polyOutput = execSync('node benchmark/bench-polyfill.ts', {
  encoding: 'utf-8',
  stdio: ['pipe', 'pipe', 'pipe'],
  cwd: process.cwd(),
})
const polyTime = performance.now() - polyStart
const polyResults: Results = JSON.parse(polyOutput.trim())
console.log(chalk.green(`  Done in ${(polyTime / 1000).toFixed(2)}s`))

// Print results
console.log(chalk.bold.cyan('\n' + '─'.repeat(70)))
console.log(chalk.bold.cyan(' Results'))
console.log(chalk.bold.cyan('─'.repeat(70)))

console.log()
console.log(chalk.gray(`  Video: ${napiResults.resolution}, ${napiResults.totalFrames} frames`))
console.log()

console.log(chalk.white('  Metric          │ napi-rs       │ polyfill      │ Comparison'))
console.log(chalk.gray('  ──────────────── ┼───────────────┼───────────────┼──────────────────'))

console.log(
  `  Decode          │ ${napiResults.decodeFps.toFixed(1).padStart(10)} fps │ ${polyResults.decodeFps.toFixed(1).padStart(10)} fps │ ${speedup(napiResults.decodeFps, polyResults.decodeFps)}`,
)
console.log(
  `  Encode          │ ${napiResults.encodeFps.toFixed(1).padStart(10)} fps │ ${polyResults.encodeFps.toFixed(1).padStart(10)} fps │ ${speedup(napiResults.encodeFps, polyResults.encodeFps)}`,
)
console.log(
  `  Encode quality  │ ${(napiResults.encodeBytesPerFrame / 1024).toFixed(1).padStart(7)} KB/fr │ ${(polyResults.encodeBytesPerFrame / 1024).toFixed(1).padStart(7)} KB/fr │ ${chalk.gray(`${(napiResults.encodeBytesPerFrame / polyResults.encodeBytesPerFrame).toFixed(1)}x more data`)}`,
)
console.log()
console.log(chalk.gray('  Note: Higher bytes/frame = higher quality output at same bitrate setting'))

console.log(chalk.bold.white('\n' + '='.repeat(70)))
console.log(chalk.bold.white(' Benchmark Complete'))
console.log(chalk.bold.white('='.repeat(70) + '\n'))
