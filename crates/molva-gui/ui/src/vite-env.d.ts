// SPDX-License-Identifier: MIT
// Types for the imports that Vite handles: `import "./styles.css"` and assets.
// TypeScript 7 checks side-effect imports by default (noUncheckedSideEffectImports).
// Without these declarations, such an import is error TS2882
/// <reference types="vite/client" />

// The window shares the kit's canvas half with the site: the vendored file is plain JavaScript
// with no declaration of its own, and this is the whole of the surface the window uses
declare module "*ddlc-cloud.js" {
  export function color(value: string): string;
  export function bayerThresholds(n: number): number[][];
  export function snapRect(
    ctx: CanvasRenderingContext2D,
    x: number,
    y: number,
    w: number,
    h: number,
  ): void;
  export function ditherDisc(
    ctx: CanvasRenderingContext2D,
    options: {
      x0?: number;
      y0?: number;
      pitch: number;
      cx: number;
      cy: number;
      r: number;
      peak?: number;
      fill: string;
      phase?: number;
    },
  ): void;
}
