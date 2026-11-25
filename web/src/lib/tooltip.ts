const tip = document.createElement('div')
tip.setAttribute('role', 'tooltip')
tip.className =
  'pointer-events-none fixed z-[70] max-w-72 rounded-lg border border-line bg-raised px-2 py-1 text-[12px] leading-snug text-ink opacity-0 shadow-[var(--shadow)] transition-opacity duration-100'
document.body.appendChild(tip)

let target: HTMLElement | null = null
let timer: number | undefined

function adopt(el: HTMLElement): string | null {
  const title = el.getAttribute('title')
  if (title) {
    if (!el.hasAttribute('aria-label') && !el.textContent?.trim()) el.setAttribute('aria-label', title)
    el.setAttribute('data-tip', title)
    el.removeAttribute('title')
  }
  return el.getAttribute('data-tip')
}

function place(el: HTMLElement) {
  const r = el.getBoundingClientRect()
  const t = tip.getBoundingClientRect()
  const below = r.bottom + 6 + t.height < window.innerHeight
  const top = below ? r.bottom + 6 : r.top - 6 - t.height
  const left = Math.min(Math.max(6, r.left + r.width / 2 - t.width / 2), window.innerWidth - t.width - 6)
  tip.style.top = `${top}px`
  tip.style.left = `${left}px`
}

function hide() {
  clearTimeout(timer)
  target = null
  tip.style.opacity = '0'
}

function show(el: HTMLElement, delay: number) {
  const text = adopt(el)
  if (!text) return
  clearTimeout(timer)
  target = el
  timer = window.setTimeout(() => {
    if (target !== el || !el.isConnected) return
    tip.textContent = text
    tip.style.opacity = '0'
    tip.style.top = '-1000px'
    requestAnimationFrame(() => {
      if (target !== el) return
      place(el)
      tip.style.opacity = '1'
    })
  }, delay)
}

document.addEventListener('pointerover', (e) => {
  if (e.pointerType === 'touch') return
  const el = (e.target as Element | null)?.closest?.('[title], [data-tip]') as HTMLElement | null
  if (!el) {
    if (target) hide()
    return
  }
  if (el === target) return
  show(el, tip.style.opacity === '1' ? 0 : 450)
})
document.addEventListener('pointerout', (e) => {
  if (target && !(e.relatedTarget instanceof Node && target.contains(e.relatedTarget))) hide()
})
document.addEventListener('pointerdown', hide, true)
document.addEventListener('keydown', hide, true)
document.addEventListener('scroll', hide, true)
document.addEventListener('focusin', (e) => {
  const el = e.target as HTMLElement
  if (el.matches?.(':focus-visible') && (el.hasAttribute('title') || el.hasAttribute('data-tip'))) show(el, 300)
})
document.addEventListener('focusout', hide)
