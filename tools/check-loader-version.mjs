// The generated index.js embeds the package version in its binding version
// check. Fail before publishing if that copy does not match package.json, so
// a release cannot ship a loader that rejects its own native packages.
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

const root = process.cwd()
const { version } = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'))
const loader = await readFile(join(root, 'index.js'), 'utf8')
const embedded = [
  ...loader.matchAll(/bindingPackageVersion !== '([^']+)'/g),
  ...loader.matchAll(/version mismatch, expected (\S+) but got/g),
].map((match) => match[1])

assert(embedded.length > 0, 'index.js has no binding version check; the loader format changed')
const stale = [...new Set(embedded.filter((found) => found !== version))]
assert.deepEqual(stale, [], `index.js expects ${stale.join(', ')} but package.json is ${version}; rebuild index.js`)
console.log(`index.js binding version check matches package.json (${version}, ${embedded.length} places)`)
