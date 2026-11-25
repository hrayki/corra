const PASSIVE = new Set(['BODY', 'BUTTON', 'A', 'SUMMARY'])

function composer(): HTMLTextAreaElement | null {
  for (const ta of document.querySelectorAll<HTMLTextAreaElement>('textarea[data-composer]'))
    if (!ta.disabled && ta.offsetParent !== null) return ta
  return null
}

window.addEventListener('keydown', (e) => {
  if (e.defaultPrevented || e.isComposing || e.ctrlKey || e.metaKey || e.altKey || e.key.length !== 1) return
  const at = document.activeElement as HTMLElement | null
  if (at && !PASSIVE.has(at.tagName)) return
  if (e.key === ' ' && at && at.tagName !== 'BODY') return
  if (document.querySelector('[role="menu"], [role="dialog"]')) return
  const ta = composer()
  if (!ta) return
  ta.focus()
  ta.setSelectionRange(ta.value.length, ta.value.length)
})
