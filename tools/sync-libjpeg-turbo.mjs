// Vendor the libjpeg-turbo that Chrome ships (chromium/deps/libjpeg_turbo) into
// vendor/libjpeg-turbo: the decoder sources and the x64, arm64 and arm SIMD code that
// build.rs compiles, the files they include, the licence files and README.chromium
// (Chromium's changes to the IJG files), Chromium's LICENSE as CHROMIUM_LICENSE, and
// CHROMIUM_PIN, which records the Chrome version, the DEPS revision, the selection rule
// and the source lists for build.rs.
// LICENSE.md, README.ijg, CHROMIUM_LICENSE and simd/nasm/jsimdext.inc are also
// copied to licenses/ and listed in licenses/manifest.json for the npm packages.
//
//   node tools/sync-libjpeg-turbo.mjs                    re-sync the pinned revision
//   node tools/sync-libjpeg-turbo.mjs --chrome <version> sync the revision in that Chrome's DEPS
//
// Vendored files are never edited by hand: the script replaces them and prints every change.
// Re-syncing the pinned revision changes nothing.
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, posix } from 'node:path'

const vendor = join(import.meta.dirname, '..', 'vendor', 'libjpeg-turbo')
const licenses = join(import.meta.dirname, '..', 'licenses')
const pinFile = 'CHROMIUM_PIN'
const headers = { 'user-agent': 'sync-libjpeg-turbo' }
const gitiles = 'https://chromium.googlesource.com'

const args = process.argv.slice(2)
assert(
  args.length === 0 || (args.length === 2 && args[0] === '--chrome' && /^\d+(\.\d+){3}$/.test(args[1])),
  'Usage: node tools/sync-libjpeg-turbo.mjs [--chrome <version>]',
)

const pin = Object.fromEntries(
  (await readFile(join(vendor, pinFile), 'utf8').catch(() => ''))
    .split('\n')
    .map((line) => line.match(/^([\w-]+): (.*)$/))
    .filter(Boolean)
    .map((match) => [match[1], match[2]]),
)

const fetchOk = async (url) => {
  const response = await fetch(url, { headers })
  assert(response.ok, `${url}: HTTP ${response.status}`)
  return response
}

const chrome = args[1] ?? pin.chrome
assert(chrome, `No ${pinFile}: pass --chrome <version>`)
// Gitiles serves raw files base64-encoded with ?format=TEXT.
const chromiumFile = async (path) => {
  const response = await fetchOk(`${gitiles}/chromium/src/+/refs/tags/${chrome}/${path}?format=TEXT`)
  return Buffer.from(await response.text(), 'base64')
}
let revision = pin['deps-revision']
if (args.length) {
  const deps = (await chromiumFile('DEPS')).toString('utf8')
  const at = deps.indexOf("'src/third_party/libjpeg_turbo'")
  assert(at >= 0, `Chrome ${chrome} DEPS has no src/third_party/libjpeg_turbo`)
  revision = deps.slice(at, at + 400).match(/libjpeg_turbo\.git'\s*\+\s*'@'\s*\+\s*'([0-9a-f]{40})'/)?.[1]
  assert(revision, `Cannot read the libjpeg_turbo revision from Chrome ${chrome} DEPS`)
}

const tree = await mkdtemp(join(tmpdir(), 'libjpeg-turbo-'))
try {
  const archive = join(tree, 'tree.tar.gz')
  const url = `${gitiles}/chromium/deps/libjpeg_turbo/+archive/${revision}.tar.gz`
  await writeFile(archive, Buffer.from(await (await fetchOk(url)).arrayBuffer()))
  execFileSync('tar', ['-xzf', archive, '-C', tree])

  // BUILD.gn builds three precisions: libjpeg12_sources = libjpeg16_sources + [...] and
  // libjpeg_sources = libjpeg12_sources + [...]. Keep the decoder side of each list.
  const gn = await readFile(join(tree, 'BUILD.gn'), 'utf8')
  const encoderOnly = /^(jc(?!omapi$)\w*|jf\w*|jdatadst|jpeg_nbits)$/
  const sources = (name) => {
    const found = gn.match(new RegExp(`^${name} = (?:(\\w+) \\+ )?\\[([^\\]]*)\\]`, 'm'))
    assert(found, `BUILD.gn has no ${name} list`)
    const own = [...found[2].matchAll(/"src\/(\w+)\.c"/g)].map((match) => match[1])
    return [...(found[1] ? sources(found[1]) : []), ...own]
  }
  const lists = Object.fromEntries(
    [
      ['bits16', 'libjpeg16_sources'],
      ['bits12', 'libjpeg12_sources'],
      ['bits8', 'libjpeg_sources'],
    ].map(([key, name]) => [key, sources(name).filter((file) => !encoderOnly.test(file))]),
  )

  // The SIMD code that Chromium links into the 8-bit library, per CPU: the NASM sources of
  // simd_asm and simd/x86_64/jsimd.c on x64, the Neon intrinsics of the simd target plus its
  // aarch64/ or aarch32/ files on arm64 and arm. The upstream dispatchers (jsimd.c) call the
  // encoder kernels too, so the whole set is kept.
  const block = (text, start) => {
    const at = text.indexOf(start)
    assert(at >= 0, `BUILD.gn has no ${start}`)
    const open = text.indexOf('{', at)
    for (let end = open, depth = 0; end < text.length; end++) {
      depth += text[end] === '{' ? 1 : text[end] === '}' ? -1 : 0
      if (!depth) return text.slice(open + 1, end)
    }
    assert.fail(`BUILD.gn: no end to ${start}`)
  }
  const strings = (text, assignment) =>
    [...text.matchAll(new RegExp(`\\b${assignment} \\[([^\\]]*)\\]`, 'g'))].flatMap((list) =>
      [...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]),
    )
  const asm = block(gn, 'nasm_assemble("simd_asm")')
  const asmX64 = block(asm, 'else if (current_cpu == "x64")')
  const simd = block(gn, 'static_library("simd")')
  const arm = block(simd, 'else if (current_cpu == "arm" ||')
  const neon = strings(arm, 'sources =')
  Object.assign(lists, {
    'simd-x86_64': [
      ...strings(block(simd, 'else if (current_cpu == "x64")'), 'sources ='),
      ...strings(asmX64, 'sources ='),
    ],
    'simd-aarch64': [...neon, ...strings(block(arm, 'else if (current_cpu == "arm64"'), 'sources \\+=')],
    'simd-arm': [...neon, ...strings(block(arm, 'if (current_cpu == "arm")'), 'sources \\+=')],
  })
  // NASM and the Neon files find some includes through these, not next to the includer.
  const includeDirs = [
    ...strings(asm, 'include_dirs ='),
    ...strings(asmX64, 'include_dirs \\+='),
    ...strings(arm, 'include_dirs \\+='),
  ]

  // Every file a compiled source includes with "..." (C) or %include (NASM), followed through
  // headers, the .c files that jdcolor.c and jdmerge.c include and the *ext*.asm files,
  // whatever #if guards them. jsimd.h, jsimddct.h and the SIMD files point to the zlib notice
  // in simd/nasm/jsimdext.inc, and jpeglibmangler.h to Chromium's LICENSE.
  const wanted = new Map([['CHROMIUM_LICENSE', await chromiumFile('LICENSE')]])
  const queue = [
    'LICENSE.md',
    'README.ijg',
    'README.chromium',
    ...lists.bits8.map((file) => `src/${file}.c`),
    ...['simd-x86_64', 'simd-aarch64', 'simd-arm'].flatMap((key) => lists[key]),
  ]
  const resolve = async (file, include) => {
    for (const dir of [posix.dirname(file), ...includeDirs]) {
      const path = posix.normalize(posix.join(dir, include))
      if (await readFile(join(tree, path)).then(Boolean, () => false)) return path
    }
    assert.fail(`${file} includes ${include}, which is not in the tree`)
  }
  while (queue.length) {
    const file = queue.pop()
    if (wanted.has(file)) continue
    const data = await readFile(join(tree, file))
    wanted.set(file, data)
    if (!/\.(c|h|asm|inc)$/.test(file)) continue
    for (const [, include] of data.toString('latin1').matchAll(/^\s*[#%]\s*include\s+"([^"]+)"/gm)) {
      queue.push(await resolve(file, include))
    }
  }

  const readme = wanted.get('README.chromium').toString('utf8')
  const version = readme.match(/^Version: (.+)$/m)?.[1]
  const upstream = readme.match(/^Revision: (.+)$/m)?.[1]
  assert(version && upstream, 'README.chromium has no Version or Revision line')
  const pinText = [
    '# libjpeg-turbo as Chrome ships it. Written by tools/sync-libjpeg-turbo.mjs: do not edit.',
    '# build.rs compiles bits8 as is and bits12/bits16 again with BITS_IN_JSAMPLE=12/16,',
    '# and simd-<Rust target arch> with WITH_SIMD on that arch (.asm with NASM).',
    `chrome: ${chrome}`,
    `deps-revision: ${revision}`,
    `source: ${url}`,
    `version: ${version}`,
    `upstream-revision: ${upstream}`,
    'rule: BUILD.gn libjpeg16_sources, libjpeg12_sources and libjpeg_sources without the',
    'rule: compressor files (src/jc*.c but src/jcomapi.c, src/jf*.c, src/jdatadst.c,',
    'rule: src/jpeg_nbits.c), the x64 sources of simd_asm and simd, the arm64 and arm',
    'rule: sources of simd, every file they include, LICENSE.md, README.ijg and',
    "rule: README.chromium (Chromium's changes);",
    `rule: CHROMIUM_LICENSE is chromium/src LICENSE at ${chrome} (for jpeglibmangler.h)`,
    ...Object.entries(lists).map(([key, files]) => `${key}: ${files.join(' ')}`),
    '',
  ].join('\n')
  wanted.set(pinFile, Buffer.from(pinText))

  const have = new Map()
  for (const entry of await readdir(vendor, { recursive: true, withFileTypes: true }).catch(() => [])) {
    if (!entry.isFile()) continue
    const path = join(entry.parentPath, entry.name)
    have.set(posix.join(...path.slice(vendor.length + 1).split(/[\\/]/)), await readFile(path))
  }
  const changes = []
  for (const [file, data] of wanted) {
    const old = have.get(file)
    if (old?.equals(data)) continue
    await mkdir(dirname(join(vendor, file)), { recursive: true })
    await writeFile(join(vendor, file), data)
    changes.push(`${old ? 'updated' : 'added'} ${file}`)
  }
  for (const file of have.keys()) {
    if (wanted.has(file)) continue
    await rm(join(vendor, file))
    changes.push(`removed ${file}`)
  }

  const manifest = JSON.parse(await readFile(join(licenses, 'manifest.json'), 'utf8'))
  const manifestText = JSON.stringify(manifest, null, 2) + '\n'
  const libjpegTurbo = {
    project: 'libjpeg-turbo',
    upstream: `${gitiles}/chromium/deps/libjpeg_turbo`,
    revision,
    ref: `Chrome ${chrome} DEPS`,
  }
  const noticeSources = [
    { ...libjpegTurbo, file: 'libjpeg-turbo-LICENSE.md.txt', upstreamPath: 'LICENSE.md' },
    { ...libjpegTurbo, file: 'libjpeg-turbo-README.ijg.txt', upstreamPath: 'README.ijg' },
    {
      file: 'libjpeg-turbo-CHROMIUM_LICENSE.txt',
      project: 'Chromium',
      upstream: `${gitiles}/chromium/src`,
      ref: `Chrome ${chrome}`,
      upstreamPath: 'LICENSE',
      wantedPath: 'CHROMIUM_LICENSE',
    },
    {
      ...libjpegTurbo,
      file: 'libjpeg-turbo-jsimdext.inc.txt',
      upstreamPath: 'simd/nasm/jsimdext.inc',
    },
  ]
  for (const { file, wantedPath, ...metadata } of noticeSources) {
    const data = wanted.get(wantedPath ?? metadata.upstreamPath)
    const old = await readFile(join(licenses, file)).catch(() => null)
    if (!old?.equals(data)) {
      await writeFile(join(licenses, file), data)
      changes.push(`${old ? 'updated' : 'added'} licenses/${file}`)
    }
    const entry = {
      file,
      ...metadata,
      excerpt: false,
      sha256: createHash('sha256').update(data).digest('hex'),
    }
    const at = manifest.files.findIndex((notice) => notice.file === file)
    manifest.files.splice(at < 0 ? manifest.files.length : at, at < 0 ? 0 : 1, entry)
  }
  if (JSON.stringify(manifest, null, 2) + '\n' !== manifestText) {
    await writeFile(join(licenses, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n')
    changes.push('updated licenses/manifest.json')
  }
  console.log(changes.length ? changes.sort().join('\n') : 'No changes.')
  console.log(`vendor/libjpeg-turbo: Chrome ${chrome}, chromium/deps/libjpeg_turbo@${revision} (${version})`)
} finally {
  await rm(tree, { recursive: true, force: true })
}
