// SPDX-License-Identifier: MIT
// The behaviour the vendored DDLC kit brings, wired to this site: the pointer cloud behind
// the page, the cloud of light at the top of the hero, the theme button and the spectrogram
// pop-up. Every canvas reads its colours from the --ddlc-* roles through the kit's color()
// probe, so a theme change recolours all of them. Without this module the pages still read
// and the theme stored before the first paint still applies; only the canvases stay empty

import { initTheme } from "./vendor/ddlc/ddlc-theme.js"
import { startCloud, color, ditherDisc, bayerThresholds } from "./vendor/ddlc/ddlc-cloud.js"

const THEME_KEY = "molvai-theme"
const STILL = matchMedia("(prefers-reduced-motion: reduce)")
const BAYER = bayerThresholds(8)

function role(name) {
  return color(name)
}

// ---------- the pointer cloud ----------

const cloudCanvas = document.querySelector(".cloud")
const cloud = cloudCanvas ? startCloud(cloudCanvas, { pitch: 6, radius: 170 }) : null

// ---------- the cloud of light at the top of the hero ----------

// A band of lit pixels behind the hero. Three discs of the accent roles drift on slow sine
// paths, and the dithering phase steps a few times a second, so the light moves without any
// element on the page moving. Under reduced motion one frame is drawn and kept
const sky = (() => {
  const canvas = document.querySelector(".sky")
  if (!canvas) return { recolor() {}, start() {}, stop() {} }

  const ctx = canvas.getContext("2d")
  let frame = 0
  let started = 0
  let size = { w: 0, h: 0 }
  let fills = []

  function read() {
    fills = [role("--ddlc-accent"), role("--ddlc-accent-live"), role("--ddlc-line")]
  }

  function paint(at) {
    const dpr = Math.min(devicePixelRatio || 1, 2)
    const w = canvas.clientWidth
    const h = canvas.clientHeight
    if (!w || !h) return
    if (size.w !== w || size.h !== h) {
      size = { w, h }
      canvas.width = Math.round(w * dpr)
      canvas.height = Math.round(h * dpr)
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    }
    ctx.clearRect(0, 0, w, h)
    const k = at / 1000
    const base = Math.min(w, h)
    const phase = Math.floor(k * 1.6) & 7
    // Three lights along the top edge, sparse enough that a sentence stays legible over them
    const discs = [
      { x: w * (0.24 + 0.06 * Math.sin(k * 0.11)), y: h * (0.06 + 0.05 * Math.cos(k * 0.09)), r: base * (0.46 + 0.05 * Math.sin(k * 0.13)), fill: fills[0], peak: 0.24, phase },
      { x: w * (0.58 + 0.07 * Math.cos(k * 0.08)), y: h * (-0.02 + 0.05 * Math.sin(k * 0.12)), r: base * (0.56 + 0.06 * Math.cos(k * 0.1)), fill: fills[1], peak: 0.2, phase: phase + 3 },
      { x: w * (0.92 + 0.05 * Math.sin(k * 0.14)), y: h * (0.04 + 0.04 * Math.sin(k * 0.07)), r: base * (0.38 + 0.04 * Math.sin(k * 0.09)), fill: fills[2], peak: 0.18, phase: phase + 5 },
    ]
    for (const disc of discs) {
      ditherDisc(ctx, { pitch: 6, cx: disc.x, cy: disc.y, r: disc.r, peak: disc.peak, fill: disc.fill, phase: disc.phase })
    }
  }

  function loop(now) {
    if (!started) started = now
    paint(now - started)
    frame = requestAnimationFrame(loop)
  }

  function start() {
    if (!fills.length) read()
    if (STILL.matches) {
      paint(0)
      return
    }
    if (frame || document.hidden) return
    frame = requestAnimationFrame(loop)
  }

  function stop() {
    if (frame) cancelAnimationFrame(frame)
    frame = 0
  }

  addEventListener("resize", () => {
    if (STILL.matches) paint(0)
  })

  return {
    recolor() {
      read()
      paint(started ? performance.now() - started : 0)
    },
    start,
    stop,
  }
})()

// ---------- the spectrogram pop-up ----------

// A small instrument: a lattice of lit cells, each one a frequency bin of a moving signal.
// It is drawn, not measured, and it never opens the microphone. The frame loop runs only
// while the dialog is open, and stops on close, on Escape and on a hidden tab
const spectro = (() => {
  const dialog = document.getElementById("spectro")
  const canvas = document.getElementById("spectro-canvas")
  const trigger = document.querySelector("[data-spectro]")
  if (!dialog || !canvas || !trigger) return { recolor() {} }

  const ctx = canvas.getContext("2d")
  const PITCH = 8
  const GAP = 2
  let frame = 0
  let started = 0
  let readout = document.getElementById("spectro-readout")
  let palette = {}

  function read() {
    palette = {
      ground: role("--ddlc-code-ground"),
      lit: role("--ddlc-accent"),
      peak: role("--ddlc-accent-live"),
      tick: role("--ddlc-grid"),
    }
  }

  function level(i, columns, k) {
    const x = i / columns
    const body = 0.5 + 0.5 * Math.sin(k * 1.3 - i * 0.42)
    const swell = 0.55 + 0.45 * Math.sin(k * 0.5 + i * 0.13)
    const spike = Math.pow(0.5 + 0.5 * Math.sin(k * 2.1 - i * 0.9), 12)
    const formant = Math.exp(-Math.pow((x - 0.24) * 3.2, 2)) * 0.5
    const hiss = Math.exp(-Math.pow((x - 0.8) * 6, 2)) * 0.22
    return Math.min(1, 0.16 + 0.28 * body * swell + 0.44 * spike + (formant + hiss) * swell)
  }

  function paint(at) {
    const dpr = Math.min(devicePixelRatio || 1, 2)
    const w = canvas.clientWidth
    if (!w) return
    const h = Math.round(w * (240 / 720))
    if (canvas.width !== Math.round(w * dpr)) {
      canvas.width = Math.round(w * dpr)
      canvas.height = Math.round(h * dpr)
      canvas.style.height = h + "px"
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    }
    ctx.fillStyle = palette.ground
    ctx.fillRect(0, 0, w, h)

    const columns = Math.floor(w / PITCH)
    const rows = Math.floor(h / PITCH)
    const k = at / 1000
    const phase = Math.floor(k * 2) & 7
    let loudest = { level: 0, column: 0 }

    for (let i = 0; i < columns; i++) {
      const value = level(i, columns, k)
      if (value > loudest.level) loudest = { level: value, column: i }
      const lit = Math.round(value * rows)
      for (let row = 0; row < rows; row++) {
        const x = i * PITCH
        const y = h - (row + 1) * PITCH
        const density = value - row / rows
        if (row < lit && density * 1.4 > BAYER[row & 7][(i + phase) & 7]) {
          ctx.fillStyle = row >= lit - 2 ? palette.peak : palette.lit
          ctx.fillRect(x, y, PITCH - GAP, PITCH - GAP)
        } else if (row % 4 === 0) {
          ctx.fillStyle = palette.tick
          ctx.fillRect(x, y + PITCH - GAP - 1, 2, 2)
        }
      }
    }

    if (readout) {
      const seconds = (at / 1000).toFixed(1).padStart(4, "0")
      const bin = Math.round((loudest.column / Math.max(1, columns)) * 64)
      readout.textContent = `t ${seconds}s · bin ${String(bin).padStart(2, "0")}`
    }
  }

  function loop(now) {
    if (!started) started = now
    paint(now - started)
    frame = requestAnimationFrame(loop)
  }

  function start() {
    if (!palette.lit) read()
    if (STILL.matches) {
      paint(0)
      return
    }
    if (frame) return
    frame = requestAnimationFrame(loop)
  }

  function stop() {
    if (frame) cancelAnimationFrame(frame)
    frame = 0
  }

  trigger.addEventListener("click", () => {
    dialog.showModal()
    start()
  })
  dialog.addEventListener("close", stop)
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) stop()
    else if (dialog.open) start()
  })

  return {
    recolor() {
      read()
      if (dialog.open) paint(started ? performance.now() - started : 0)
    },
  }
})()

// ---------- the theme button ----------

const themeButton = document.getElementById("theme")
const themeIcon = document.getElementById("theme-icon")

function themeLabel(mode) {
  const ru = document.documentElement.lang === "ru"
  const label = ru ? `Тема: ${mode}` : `Theme: ${mode}`
  themeButton.setAttribute("aria-label", label)
  themeButton.title = label
}

const theme = themeButton && themeIcon
  ? initTheme({
      button: themeButton,
      icon: themeIcon,
      key: THEME_KEY,
      onChange: (mode) => {
        if (cloud) cloud.recolor()
        sky.recolor()
        spectro.recolor()
        themeLabel(mode)
      },
    })
  : null

document.addEventListener("molvai:lang", () => {
  if (theme) themeLabel(theme.mode)
})

sky.start()
