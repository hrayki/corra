export function splitMarkdownBlocks(text: string): string[] {
  if (!text) return []
  const lines = text.split('\n')
  const blocks: string[] = []
  let current: string[] = []
  let fenceChar = ''
  let fenceLen = 0

  for (const line of lines) {
    const m = line.match(/^\s{0,3}(`{3,}|~{3,})/)
    if (!fenceChar) {
      if (m) {
        fenceChar = m[1][0]
        fenceLen = m[1].length
        current.push(line)
        continue
      }
      if (line.trim() === '') {
        if (current.length) {
          blocks.push(current.join('\n'))
          current = []
        }
        continue
      }
      current.push(line)
    } else {
      current.push(line)
      const close = line.match(new RegExp(`^\\s{0,3}${fenceChar}{${fenceLen},}\\s*$`))
      if (close) {
        fenceChar = ''
        fenceLen = 0
      }
    }
  }
  if (current.length) blocks.push(current.join('\n'))
  return blocks
}
