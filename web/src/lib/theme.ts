import { createSignal } from 'solid-js'

export type ThemeChoice = 'auto' | 'light' | 'dark'

const KEY = 'corra_theme'
const saved = localStorage.getItem(KEY)
const initial: ThemeChoice = saved === 'light' || saved === 'dark' ? saved : 'auto'
const [choice, setChoiceSignal] = createSignal<ThemeChoice>(initial)
const [mode, setMode] = createSignal<'light' | 'dark'>('light')
export { choice, mode }

const media = matchMedia('(prefers-color-scheme: dark)')

function apply() {
  const c = choice()
  const m = c === 'light' || c === 'dark' ? c : media.matches ? 'dark' : 'light'
  document.documentElement.dataset.theme = m
  setMode(m)
}

export function setChoice(c: ThemeChoice) {
  localStorage.setItem(KEY, c)
  setChoiceSignal(c)
  apply()
}

media.addEventListener('change', apply)
apply()
