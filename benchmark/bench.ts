import { Bench } from 'tinybench'

import { plus100 } from '../index.js'

function add(a: number) {
  return a + 100
}

const bench = new Bench()

bench.add('Native a + 100', () => {
  plus100(10)
})

bench.add('JavaScript a + 100', () => {
  add(10)
})

await bench.run()

console.table(bench.table())
