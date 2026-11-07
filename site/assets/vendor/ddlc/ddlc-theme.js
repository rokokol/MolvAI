// Copied by generate.sh from src/theme.js — do not edit
// The theme button of the DDLC web kit: it cycles system, light and dark, shows an 8x8 pixel
// icon for each, and keeps the choice in localStorage under the key the page names. "system"
// leaves data-theme unset, so the stylesheet follows the OS.
// A page applies the stored choice before the first paint, or it flashes the other side
// first. That is a few lines in <head>, the same for every page but the key:
//
//   <script>
//     try {
//       const t = localStorage.getItem("my-page:theme")
//       if (t === "light" || t === "dark") document.documentElement.dataset.theme = t
//     } catch {}
//   </script>

export const MODES = ['system', 'light', 'dark']

// A monitor, a sun and a moon, each a path on an 8x8 grid drawn with fill-rule evenodd
export const ICONS = {
  system: 'M0 1h8v5H0zM1 2v3h6V2zM3 6h2v1H3zM2 7h4v1H2z',
  light: 'M3 0h2v1H3zM3 7h2v1H3zM0 3h1v2H0zM7 3h1v2H7zM2 2h4v4H2zM1 1h1v1H1zM6 1h1v1H6zM1 6h1v1H1zM6 6h1v1H6z',
  dark: 'M2 0h3v1H2zM1 1h2v1H1zM0 2h2v4H0zM1 6h2v1H1zM2 7h4v1H2zM5 6h2v1H5zM6 5h2v1H6z',
}

function stored(key) {
  try {
    const t = localStorage.getItem(key)
    return MODES.includes(t) ? t : 'system'
  } catch {
    return 'system'
  }
}

// button: the element that cycles; icon: the <path> inside its <svg viewBox="0 0 8 8">;
// key: the localStorage key; onChange: runs after every change of the side shown, including
// one the OS makes while the mode is "system", so canvases can read their colours again
export function initTheme({ button, icon, key, onChange = () => {} }) {
  let mode = stored(key)
  const apply = () => {
    const root = document.documentElement
    if (mode === 'system') delete root.dataset.theme
    else root.dataset.theme = mode
    icon.setAttribute('d', ICONS[mode])
    icon.setAttribute('fill-rule', 'evenodd')
    button.setAttribute('aria-label', `Theme: ${mode}`)
    button.title = `Theme: ${mode}`
    onChange(mode)
  }
  button.addEventListener('click', () => {
    mode = MODES[(MODES.indexOf(mode) + 1) % MODES.length]
    try {
      if (mode === 'system') localStorage.removeItem(key)
      else localStorage.setItem(key, mode)
    } catch {}
    apply()
  })
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (mode === 'system') onChange(mode)
  })
  apply()
  return { get mode() { return mode } }
}
