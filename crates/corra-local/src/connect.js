const $ = (id) => document.getElementById(id)
  const current = $('u').value
  let code = false
  if (current) {
    $('title').textContent = 'Use another hub'
    $('back').hidden = false
    $('back').querySelector('a').href = current
  }
  $('swap').addEventListener('click', (ev) => {
    ev.preventDefault()
    code = !code
    $('by-key').hidden = code
    $('by-code').hidden = !code
    $('swap').textContent = code ? 'Use the access key instead' : 'Use a pairing code instead'
  })
  $('f').addEventListener('submit', async (ev) => {
    ev.preventDefault()
    $('e').textContent = ''
    $('go').disabled = true
    $('go').textContent = 'Connecting…'
    try {
      const r = await fetch(location.href, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ url: $('u').value, key: code ? '' : $('k').value, code: code ? $('c').value : '' }),
      })
      const j = await r.json().catch(() => ({}))
      if (!r.ok) throw new Error(j.error || 'Something went wrong.')
      $('go').textContent = 'Connected. Opening your hub…'
    } catch (err) {
      $('e').textContent = err.message
      $('go').disabled = false
      $('go').textContent = 'Connect'
    }
  })
