import { readFile, writeFile } from 'node:fs/promises'

import { NapiCli, createBuildCommand } from '@napi-rs/cli'
import { format, type FormatOptions } from 'oxfmt'

import oxfmtConfig from './.oxfmtrc.json' with { type: 'json' }

const buildCommand = createBuildCommand(process.argv.slice(2))
const cli = new NapiCli()
const buildOptions = buildCommand.getOptions()
const { task } = await cli.build(buildOptions)
const outputs = await task

for (const output of outputs) {
  if (output.kind !== 'node') {
    const { code } = await format(output.path, await readFile(output.path, 'utf-8'), oxfmtConfig as FormatOptions)
    await writeFile(output.path, code)
  }
}
