import { test } from 'node:test'
import assert from 'node:assert/strict'
import { fileInCode, fileInHref, inside, joinPath } from '../src/lib/filelinks.ts'

test('code spans an agent writes for files become links', () => {
  assert.deepEqual(fileInCode('crates/corra-hub/src/ws.rs:1220'), { path: 'crates/corra-hub/src/ws.rs', line: 1220 })
  assert.deepEqual(fileInCode('web/src/ui/Panel.tsx:40-52'), { path: 'web/src/ui/Panel.tsx', line: 40 })
  assert.deepEqual(fileInCode('src/main.rs:12:5'), { path: 'src/main.rs', line: 12 })
  assert.deepEqual(fileInCode('/srv/app/Cargo.toml'), { path: '/srv/app/Cargo.toml', line: undefined })
  assert.deepEqual(fileInCode('~/.claude/settings.json'), { path: '~/.claude/settings.json', line: undefined })
  for (const f of [
    'README.md',
    'package.json',
    'Dockerfile',
    '.gitignore',
    'Cargo.lock',
    'apps/app/src-tauri/tauri.conf.json',
    'web/.env.example',
  ])
    assert.ok(fileInCode(f), f)
})

test('code that only looks a little like a path stays code', () => {
  for (const c of [
    'hub.place_project',
    'console.log',
    'process.env',
    'import.meta',
    'owner/repo',
    'Hub::new',
    '0.88em',
    '.md',
    'v1.2.3',
    'git status -s',
    'https://example.com/a.js',
    'web/src/ui/',
    'a/b',
  ])
    assert.equal(fileInCode(c), null, c)
})

test('markdown links to files open them; web links do not', () => {
  assert.deepEqual(fileInHref('web/src/ui/Panel.tsx#L360'), { path: 'web/src/ui/Panel.tsx', line: 360 })
  assert.deepEqual(fileInHref('src/a%20b.ts:7'), { path: 'src/a b.ts', line: 7 })
  assert.deepEqual(fileInHref('file:///etc/hosts'), { path: '/etc/hosts', line: undefined })
  assert.deepEqual(fileInHref('../DESIGN.md'), { path: '../DESIGN.md', line: undefined })
  for (const h of ['https://code.claude.com/docs', 'mailto:a@b.c', '#section', '//cdn.example.com/x.js', 'docs/'])
    assert.equal(fileInHref(h), null, h)
})

test('paths are resolved against the folder they were written in', () => {
  assert.equal(joinPath('docs/guide', '../DESIGN.md'), 'docs/DESIGN.md')
  assert.equal(joinPath('', './README.md'), 'README.md')
  assert.equal(joinPath('/srv/app', 'x/y.ts'), '/srv/app/x/y.ts')
  assert.equal(joinPath('docs', '/etc/hosts'), '/etc/hosts')
  assert.equal(inside('/srv/app/', '/srv/app/web/a.ts'), 'web/a.ts')
  assert.equal(inside('/srv/app', '/srv/app-old/a.ts'), '/srv/app-old/a.ts')
})
