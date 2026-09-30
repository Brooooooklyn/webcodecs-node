// Keep legal material in every generated native package, whose files list is
// independent of the root package. A successful check verifies packaging only;
// it does not verify the Corresponding Source or approve a binary release.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFile, cp, readFile, readdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { parseTriple } from '@napi-rs/cli'

const args = process.argv.slice(2)
assert(
  args.length === 0 || (args.length === 1 && args[0] === '--check'),
  'Usage: node tools/package-licenses.mjs [--check]',
)
const check = args[0] === '--check'
const root = process.cwd()
const readJson = async (path) => JSON.parse(await readFile(path, 'utf8'))
const pkg = await readJson(join(root, 'package.json'))
const license = 'SEE LICENSE IN LICENSES.md'
const documents = ['LICENSE', 'LICENSES.md', 'THIRD_PARTY_NOTICES.md', 'SOURCE_DISTRIBUTION.md']
const noticeManifest = await readJson(join(root, 'licenses/manifest.json'))
const noticePaths = noticeManifest.files.map(({ file }) => {
  assert(!file.includes('/') && !file.includes('\\') && file !== '..', `Invalid notice filename: ${file}`)
  return `licenses/${file}`
})
const requiredFiles = [...documents, 'licenses/manifest.json', ...noticePaths]
assert.equal(new Set(noticePaths).size, noticePaths.length, 'Duplicate notice in manifest')
assert.equal(pkg.license, license, 'Root package license must refer to LICENSES.md')
const expectedNoticeNames = ['manifest.json', ...noticeManifest.files.map(({ file }) => file)].sort()
assert.deepEqual((await readdir(join(root, 'licenses'))).sort(), expectedNoticeNames, 'Unlisted reference notice')
for (const notice of noticeManifest.files) {
  const data = await readFile(join(root, 'licenses', notice.file))
  assert.equal(createHash('sha256').update(data).digest('hex'), notice.sha256, `Notice digest mismatch: ${notice.file}`)
}

const suffixes = pkg.napi.targets.map((target) => parseTriple(target).platformArchABI)
assert.equal(new Set(suffixes).size, suffixes.length, 'Duplicate native target')
// Fail when generated targets are missing or stale instead of checking only
// whichever directories happen to exist.
const entries = await readdir(join(root, 'npm'), { withFileTypes: true })
const actualSuffixes = entries.filter((entry) => entry.isDirectory()).map((entry) => entry.name)
assert.deepEqual(actualSuffixes.sort(), [...suffixes].sort(), 'Run napi create-npm-dirs for the configured targets')

const marker = '\n## Licenses and source\n'
const readmeNotice = `${marker}\nThis native addon statically links GPL-enabled FFmpeg and codec libraries.\nSee [LICENSES.md](LICENSES.md), [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md),\nand [SOURCE_DISTRIBUTION.md](SOURCE_DISTRIBUTION.md) for licensing terms and\noutstanding source-distribution work. The original project's MIT license\ndoes not by itself cover this combined binary.\n`
const packages = [{ dir: root, name: pkg.name }]
for (const suffix of suffixes) {
  const dir = join(root, 'npm', suffix)
  const path = join(dir, 'package.json')
  const native = await readJson(path)
  const expectedName = `${pkg.name}-${suffix}`
  assert.equal(native.name, expectedName, `Unexpected package in ${dir}`)
  assert.equal(native.version, pkg.version, `Version mismatch: ${native.name}`)
  assert.equal(native.main, `${pkg.napi.binaryName}.${suffix}.node`, `Unexpected binary: ${native.name}`)
  assert(Array.isArray(native.files) && native.files.includes(native.main), `Missing binary in files: ${native.name}`)
  if (!check) {
    native.license = license
    // napi prepublish validates literal file entries; a directory entry fails.
    native.files = [...new Set([...native.files.filter((file) => !file.startsWith('licenses/')), ...requiredFiles])]
    for (const file of documents) await copyFile(join(root, file), join(dir, file))
    await cp(join(root, 'licenses'), join(dir, 'licenses'), { recursive: true })
    const readme = await readFile(join(dir, 'README.md'), 'utf8')
    await writeFile(join(dir, 'README.md'), readme.split(marker)[0].trimEnd() + '\n' + readmeNotice)
    await writeFile(path, JSON.stringify(native, null, 2) + '\n')
  } else {
    assert.equal(native.license, license, `Wrong license field: ${native.name}`)
    assert.deepEqual(
      (await readdir(join(dir, 'licenses'))).sort(),
      expectedNoticeNames,
      `Stale notice files: ${native.name}`,
    )
    assert(
      (await readFile(join(dir, 'README.md'), 'utf8')).endsWith(readmeNotice),
      `Missing README notice: ${native.name}`,
    )
    for (const file of requiredFiles) {
      assert.deepEqual(
        await readFile(join(dir, file)),
        await readFile(join(root, file)),
        `Missing/stale ${file}: ${native.name}`,
      )
    }
  }
  packages.push({ dir, name: native.name, binary: native.main })
}

if (check) {
  for (const { dir, name, binary } of packages) {
    // --ignore-scripts is essential: prepublishOnly invokes napi prepublish,
    // which can publish native packages. No lifecycle is run by this check.
    const command = process.platform === 'win32' ? 'npm.cmd' : 'npm'
    const packed = JSON.parse(
      execFileSync(command, ['pack', '--dry-run', '--json', '--ignore-scripts'], {
        cwd: dir,
        encoding: 'utf8',
        shell: process.platform === 'win32',
      }),
    )
    assert.equal(packed.length, 1, `Unexpected npm pack result for ${name}`)
    const files = new Set(packed[0].files.map((file) => file.path))
    for (const file of [...requiredFiles, ...(binary ? [binary] : [])]) {
      assert(files.has(file), `${name}: npm pack excludes ${file}`)
    }
    console.log(
      `Checked ${name}: ${requiredFiles.length} license/source-notice files${binary ? ' and native binary' : ''}`,
    )
  }
} else {
  console.log(
    `Prepared license notices for ${suffixes.length} native packages. Run pnpm licenses:check after artifacts are present.`,
  )
}
