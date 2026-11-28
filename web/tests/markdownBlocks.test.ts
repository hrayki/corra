import { test } from 'node:test'
import assert from 'node:assert/strict'
import { splitMarkdownBlocks } from '../src/lib/markdownBlocks.ts'

test('splits completed paragraphs while preserving blank lines inside fenced code blocks', () => {
  const md = ['First paragraph.', '', '```ts', 'const a = 1', '', 'const b = 2', '```', '', 'Tail paragraph'].join('\n')
  const blocks = splitMarkdownBlocks(md)
  assert.equal(blocks.length, 3)
  assert.equal(blocks[0], 'First paragraph.')
  assert.equal(blocks[1], '```ts\nconst a = 1\n\nconst b = 2\n```')
  assert.equal(blocks[2], 'Tail paragraph')
})

test('keeps an unclosed streaming fence intact as the final tail block', () => {
  const md = ['Intro', '', '```rust', 'fn main() {', '', '    println!("hi");'].join('\n')
  const blocks = splitMarkdownBlocks(md)
  assert.equal(blocks.length, 2)
  assert.equal(blocks[0], 'Intro')
  assert.match(blocks[1], /^```rust\nfn main/)
})
