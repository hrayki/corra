import { createSignal } from 'solid-js'

export type ThemeChoice = 'auto' | 'light' | 'dark' | 'omarchy'
type Omarchy = { name: string; mode: 'light' | 'dark'; colors: Record<string, string> }

const KEY = 'corra_theme'
const [choice, setChoiceSignal] = createSignal<ThemeChoice>((localStorage.getItem(KEY) as ThemeChoice) || 'auto')
const [omarchy, setOmarchy] = createSignal<Omarchy | null>(null)
const [mode, setMode] = createSignal<'light' | 'dark'>('light')
export { choice, omarchy, mode }

const media = matchMedia('(prefers-color-scheme: dark)')
const VARS = ['--bg', '--panel', '--raised', '--line', '--ink', '--mute', '--faint', '--accent', '--ok', '--warn', '--bad']

function apply() {
  const root = document.documentElement
  const c = choice()
  const o = omarchy()
  for (const v of VARS) root.style.removeProperty(v)
  if (c === 'omarchy' && o) {
    const k = o.colors
    const bg = k.background ?? '#111111'
    const fg = k.foreground ?? '#eeeeee'
    const mix = (a: string, b: string, p: number) => `color-mix(in srgb, ${a} ${100 - p}%, ${b} ${p}%)`
    const set: Record<string, string> = {
      '--bg': bg,
      '--panel': mix(bg, fg, o.mode === 'light' ? 5 : 4),
      '--raised': o.mode === 'light' ? mix(bg, '#ffffff', 65) : mix(bg, fg, 8),
      '--line': mix(bg, fg, 15),
      '--ink': fg,
      '--mute': mix(fg, bg, 38),
      '--faint': mix(fg, bg, 62),
      '--accent': k.accent ?? k.color4 ?? fg,
      '--ok': k.color2 ?? '#69db7c',
      '--warn': k.color3 ?? '#fcc419',
      '--bad': k.color1 ?? '#ff8787',
    }
    for (const [name, v] of Object.entries(set)) root.style.setProperty(name, v)
    root.dataset.theme = o.mode
    setMode(o.mode)
    return
  }
  const m = c === 'light' || c === 'dark' ? c : media.matches ? 'dark' : 'light'
  root.dataset.theme = m
  setMode(m)
}

export function setChoice(c: ThemeChoice) {
  localStorage.setItem(KEY, c)
  setChoiceSignal(c)
  apply()
}

export function setOmarchyTheme(o: Omarchy | null) {
  const prev = JSON.stringify(omarchy())
  if (prev === JSON.stringify(o)) return
  setOmarchy(o)
  if (o && !localStorage.getItem(KEY)) setChoiceSignal('omarchy')
  apply()
}

media.addEventListener('change', apply)
apply()
