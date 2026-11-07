// Copied by generate.sh from src/cloud.js — do not edit
// The canvas half of the DDLC web kit: reading a colour role for a canvas, the ordered
// dithering both pages draw their glows with, and the cloud, a ball of pixels behind the page
// that trails the pointer on a spring.

// A canvas needs a resolved colour, and getComputedStyle returns a custom property as it was
// written, light-dark() and all. So the value is applied to a hidden probe and read back.
// A name starting with -- is read as var(name); anything else is any CSS colour
let probe

export function color(value) {
  if (!probe) {
    probe = document.createElement('span')
    probe.hidden = true
    document.body.append(probe)
  }
  probe.style.color = value.startsWith('--') ? `var(${value})` : value
  return getComputedStyle(probe).color
}

function bayer(n) {
  if (n === 1) return [[0]]
  const m = bayer(n / 2)
  const h = n / 2
  const out = Array.from({ length: n }, () => new Array(n))
  for (let y = 0; y < h; y++)
    for (let x = 0; x < h; x++) {
      const v = 4 * m[y][x]
      out[y][x] = v
      out[y][x + h] = v + 2
      out[y + h][x] = v + 3
      out[y + h][x + h] = v + 1
    }
  return out
}

// The thresholds of an n x n Bayer matrix, each in (0, 1); n is a power of two
export function bayerThresholds(n) {
  return bayer(n).map(row => row.map(v => (v + 0.5) / (n * n)))
}

const BAYER = bayerThresholds(8)

// Fills a rectangle with its edges rounded to whole pixels: a fractional pitch then gives
// cells one pixel apart in size instead of blurred edges
export function snapRect(ctx, x, y, w, h) {
  const x0 = Math.round(x)
  const y0 = Math.round(y)
  ctx.fillRect(x0, y0, Math.round(x + w) - x0, Math.round(y + h) - y0)
}

// Lights the cells of a lattice with pitch p and origin (x0, y0) inside a disc of radius r
// about (cx, cy). A cell is lit when the density at its distance, peak * (1 - (d / r)^2),
// beats its threshold in the 8x8 Bayer matrix, so the disc's edge dissolves. A phase shifts
// the matrix across the lattice, and the same disc lights a different set of cells
export function ditherDisc(ctx, { x0 = 0, y0 = 0, pitch, cx, cy, r, peak = 1, fill, phase = 0 }) {
  if (r <= 0) return
  const gap = Math.max(1, Math.round(pitch / 4))
  ctx.fillStyle = fill
  const i0 = Math.floor((cx - r - x0) / pitch)
  const i1 = Math.ceil((cx + r - x0) / pitch)
  const j0 = Math.floor((cy - r - y0) / pitch)
  const j1 = Math.ceil((cy + r - y0) / pitch)
  for (let j = j0; j <= j1; j++)
    for (let i = i0; i <= i1; i++) {
      const dx = x0 + (i + 0.5) * pitch - cx
      const dy = y0 + (j + 0.5) * pitch - cy
      const density = peak * (1 - (dx * dx + dy * dy) / (r * r))
      if (density > BAYER[(j + phase) & 7][(i + 3 * phase) & 7])
        snapRect(ctx, x0 + i * pitch, y0 + j * pitch, pitch - gap, pitch - gap)
    }
}

// The cloud: a dithered ball on a fixed lattice that trails the pointer on a spring. It draws
// only while something moves, and not at all under prefers-reduced-motion. recolor() reads
// the role again after a theme change
export function startCloud(canvas, { pitch = 6, radius = 150, role = '--ddlc-cloud' } = {}) {
  const STIFFNESS = 0.08
  const DAMPING = 0.72
  const ctx = canvas.getContext('2d')
  const still = matchMedia('(prefers-reduced-motion: reduce)')
  const target = { x: innerWidth / 2, y: innerHeight / 2 }
  const centre = { ...target }
  const speed = { x: 0, y: 0 }
  let size = 0
  let shown = false
  let frame = 0
  let fill = color(role)

  function draw() {
    ctx.clearRect(0, 0, innerWidth, innerHeight)
    if (size >= 0.02) ditherDisc(ctx, { pitch, cx: centre.x, cy: centre.y, r: radius * size, fill })
  }

  function resize() {
    const dpr = devicePixelRatio || 1
    canvas.width = Math.round(innerWidth * dpr)
    canvas.height = Math.round(innerHeight * dpr)
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    draw()
  }

  function step() {
    speed.x = (speed.x + (target.x - centre.x) * STIFFNESS) * DAMPING
    speed.y = (speed.y + (target.y - centre.y) * STIFFNESS) * DAMPING
    centre.x += speed.x
    centre.y += speed.y
    size += ((shown ? 1 : 0) - size) * 0.12
    draw()
    const moving = Math.abs(speed.x) + Math.abs(speed.y) > 0.05 || Math.abs((shown ? 1 : 0) - size) > 0.01
    frame = moving && !document.hidden ? requestAnimationFrame(step) : 0
  }

  function kick() {
    if (!still.matches && !frame) frame = requestAnimationFrame(step)
  }

  function follow(e) {
    target.x = e.clientX
    target.y = e.clientY
    shown = true
    kick()
  }

  addEventListener('pointermove', e => {
    if (e.pointerType !== 'touch' || e.buttons) follow(e)
  })
  addEventListener('pointerdown', follow)
  addEventListener('pointerup', e => {
    if (e.pointerType !== 'touch') return
    shown = false
    kick()
  })
  document.documentElement.addEventListener('pointerleave', () => {
    shown = false
    kick()
  })
  addEventListener('resize', resize)
  still.addEventListener('change', () => {
    if (!still.matches) return kick()
    size = 0
    draw()
  })
  resize()

  return {
    recolor() {
      fill = color(role)
      draw()
    },
  }
}
