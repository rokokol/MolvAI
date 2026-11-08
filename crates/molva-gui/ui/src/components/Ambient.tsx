// SPDX-License-Identifier: MIT
// The window's ambient motion: a strip of dithered light along the top edge, and the trail of
// the pointer above the page.
//
// Both are canvases painted from one animation frame, so a pointer event appends one sample and
// creates no node. The dithering is the kit's own and every colour is a role read through the
// kit's probe, so a theme change recolours both canvases without a second palette. The strip
// repaints about nine times a second and the trail at the frame rate, and none of it exists for
// a reader who asked for less motion: that reader keeps one still frame of the strip

import { useEffect, useRef } from "react";

import {
  color,
  ditherDisc,
  snapRect,
} from "../../../../../site/assets/vendor/ddlc/ddlc-cloud.js";

import {
  liveSamples,
  pushSample,
  snapToCell,
  STRIP_CELL,
  STRIP_PHASE_MS,
  stripDiscs,
  TRAIL_CELL,
  TRAIL_INK,
  trailStrength,
  type Sample,
} from "./ambient";

/** How often the strip takes a new frame: the light drifts far slower than the screen refreshes */
const STRIP_STEP_MS = 110;

/** How often the trail takes a new frame while samples are alive */
const TRAIL_STEP_MS = 33;

/** Give a canvas the device's pixels but the layout's pixels as its coordinates */
function fitCanvas(
  canvas: HTMLCanvasElement,
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
): void {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  const pixelWidth = Math.round(width * dpr);
  const pixelHeight = Math.round(height * dpr);
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
    context.setTransform(dpr, 0, 0, dpr, 0, 0);
  }
}

export default function Ambient() {
  const stripRef = useRef<HTMLCanvasElement | null>(null);
  const trailRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const strip = stripRef.current;
    const trail = trailRef.current;
    const stripContext = strip?.getContext("2d") ?? null;
    const trailContext = trail?.getContext("2d") ?? null;
    if (!strip || !trail || !stripContext || !trailContext) {
      return;
    }

    const still = window.matchMedia("(prefers-reduced-motion: reduce)");
    const scheme = window.matchMedia("(prefers-color-scheme: dark)");
    const samples: Sample[] = [];
    let tones: string[] = [];
    let trailInk = "";
    let timer = 0;
    let lastStrip = Number.NEGATIVE_INFINITY;
    let lastTrail = Number.NEGATIVE_INFINITY;
    let stripDue = true;
    let trailDue = true;

    const readTones = () => {
      tones = [color("--ddlc-accent"), color("--ddlc-accent-live"), color("--ddlc-line")];
      trailInk = color("--ddlc-accent");
      stripDue = true;
    };

    const paintStrip = (now: number) => {
      const width = strip.clientWidth;
      const height = strip.clientHeight;
      if (!width || !height) {
        return;
      }
      fitCanvas(strip, stripContext, width, height);
      stripContext.clearRect(0, 0, width, height);
      // The matrix steps across the lattice a few times a second, so the light moves in whole
      // cells instead of sliding: the same trick the kit's spectrogram uses for its crest
      const phase = Math.floor(now / STRIP_PHASE_MS) & 7;
      for (const disc of stripDiscs(width, height, now)) {
        ditherDisc(stripContext, {
          pitch: STRIP_CELL,
          cx: disc.cx,
          cy: disc.cy,
          r: disc.r,
          peak: disc.peak,
          fill: tones[disc.tone] ?? "",
          phase: phase + disc.phase,
        });
      }
    };

    const paintTrail = (now: number) => {
      const width = trail.clientWidth;
      const height = trail.clientHeight;
      if (!width || !height) {
        return;
      }
      fitCanvas(trail, trailContext, width, height);
      trailContext.clearRect(0, 0, width, height);
      trailContext.fillStyle = trailInk;
      for (const sample of liveSamples(samples, now)) {
        trailContext.globalAlpha = trailStrength(now - sample.at) * TRAIL_INK;
        snapRect(
          trailContext,
          snapToCell(sample.x),
          snapToCell(sample.y),
          TRAIL_CELL - 1,
          TRAIL_CELL - 1,
        );
      }
      trailContext.globalAlpha = 1;
    };

    // A continuous requestAnimationFrame loop costs a full frame even when neither canvas
    // changes. Wake only when there is work: the strip at its own pace, the trail while it has
    // live samples. This keeps WebKit off the 100 % CPU ceiling on an idle Dashboard
    const loop = () => {
      timer = 0;
      if (document.hidden) {
        return;
      }
      const now = performance.now();
      let next = STRIP_STEP_MS;
      if (stripDue || now - lastStrip >= STRIP_STEP_MS) {
        paintStrip(now);
        lastStrip = now;
        stripDue = false;
      }
      if (trailDue || (samples.length > 0 && now - lastTrail >= TRAIL_STEP_MS)) {
        paintTrail(now);
        lastTrail = now;
        trailDue = false;
        next = TRAIL_STEP_MS;
      }
      if (!still.matches) {
        timer = window.setTimeout(loop, samples.length > 0 ? Math.min(next, TRAIL_STEP_MS) : next);
      }
    };

    const kick = () => {
      if (timer) {
        return;
      }
      stripDue = true;
      timer = window.setTimeout(loop, 0);
    };

    const onPointer = (event: PointerEvent) => {
      // A reader who asked for less motion keeps the one still frame of the strip and no trail
      if (still.matches) {
        return;
      }
      // A finger that slides over the page without pressing is a scroll, not a pointer to follow
      if (event.pointerType === "touch" && event.type === "pointermove") {
        return;
      }
      pushSample(samples, event.clientX, event.clientY, performance.now());
      trailDue = true;
      kick();
    };

    const onVisibility = () => {
      if (!document.hidden) {
        kick();
      }
    };

    const onThemeChange = () => {
      readTones();
      kick();
    };

    const onStillChange = () => {
      if (still.matches) {
        if (timer) {
          window.clearTimeout(timer);
          timer = 0;
        }
        samples.length = 0;
        trailContext.clearRect(0, 0, trail.clientWidth, trail.clientHeight);
        paintStrip(performance.now());
      } else {
        kick();
      }
    };

    readTones();
    if (still.matches) {
      paintStrip(performance.now());
    } else {
      kick();
    }

    window.addEventListener("pointermove", onPointer, { passive: true });
    window.addEventListener("pointerdown", onPointer, { passive: true });
    window.addEventListener("resize", kick);
    document.addEventListener("visibilitychange", onVisibility);
    scheme.addEventListener("change", onThemeChange);
    still.addEventListener("change", onStillChange);
    const themeWatch = new MutationObserver(onThemeChange);
    themeWatch.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });

    return () => {
      if (timer) {
        window.clearTimeout(timer);
      }
      themeWatch.disconnect();
      window.removeEventListener("pointermove", onPointer);
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("resize", kick);
      document.removeEventListener("visibilitychange", onVisibility);
      scheme.removeEventListener("change", onThemeChange);
      still.removeEventListener("change", onStillChange);
    };
  }, []);

  return (
    <>
      <canvas ref={stripRef} className="ambient-strip" aria-hidden="true" />
      <canvas ref={trailRef} className="ambient-trail" aria-hidden="true" />
    </>
  );
}
