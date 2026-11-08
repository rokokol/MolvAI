// SPDX-License-Identifier: MIT
// The spectrogram of an audio file, drawn from the file itself.
//
// The webview decodes the audio through Web Audio and the picture is computed here, so the
// window needs no extra process and no extra crate. Decoding a long file is a whole-file
// allocation, so a file above the limit shows a stated reason instead of a picture; the player
// and the transcript above it keep working either way.

import { useEffect, useRef, useState } from "react";

import { useI18n } from "../i18n";
import {
  computeSpectrogram,
  monoMix,
  spectrogramColor,
  type Spectrogram as SpectrogramData,
} from "./spectrogram";

/** Files above this length are not decoded: their samples would not fit the window's memory */
export const MAX_SPECTROGRAM_SECS = 900;

/** The decode rate of the picture; speech needs no more, and the allocation stays small */
const DECODE_RATE = 16_000;

type State =
  | { kind: "waiting" }
  | { kind: "loading" }
  | { kind: "ready"; spec: SpectrogramData }
  | { kind: "too-long" }
  | { kind: "error"; reason: string };

export interface SpectrogramProps {
  url: string;
  durationMs: number;
  currentMs: number;
  onSeek: (ms: number) => void;
}

export default function Spectrogram({
  url,
  durationMs,
  currentMs,
  onSeek,
}: SpectrogramProps) {
  const { t } = useI18n();
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [state, setState] = useState<State>({ kind: "waiting" });

  useEffect(() => {
    if (!(durationMs > 0)) {
      return;
    }
    if (durationMs > MAX_SPECTROGRAM_SECS * 1000) {
      setState({ kind: "too-long" });
      return;
    }
    let cancelled = false;
    setState({ kind: "loading" });
    void (async () => {
      try {
        const response = await fetch(url);
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}`);
        }
        const bytes = await response.arrayBuffer();
        // A 16 kHz context makes the decoder resample: the allocation is a third of 48 kHz
        let context: AudioContext;
        try {
          context = new AudioContext({ sampleRate: DECODE_RATE });
        } catch {
          context = new AudioContext();
        }
        let buffer: AudioBuffer;
        try {
          buffer = await context.decodeAudioData(bytes);
        } finally {
          void context.close().catch(() => undefined);
        }
        if (cancelled) {
          return;
        }
        const samples = monoMix(buffer);
        // Let the loading state paint before the frames are computed
        await new Promise((resolve) => setTimeout(resolve, 0));
        if (cancelled) {
          return;
        }
        setState({
          kind: "ready",
          spec: computeSpectrogram(samples, buffer.sampleRate, {
            cols: 900,
            rows: 120,
          }),
        });
      } catch (err) {
        if (!cancelled) {
          setState({
            kind: "error",
            reason: err instanceof Error ? err.message : String(err),
          });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [url, durationMs]);

  // Draw once per picture: the canvas holds the bitmap, CSS stretches it to the card
  useEffect(() => {
    if (state.kind !== "ready") {
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    const { spec } = state;
    canvas.width = spec.cols;
    canvas.height = spec.rows;
    const context = canvas.getContext("2d");
    if (!context) {
      return;
    }
    const image = context.createImageData(spec.cols, spec.rows);
    for (let col = 0; col < spec.cols; col += 1) {
      for (let row = 0; row < spec.rows; row += 1) {
        const value = spec.values[col * spec.rows + row];
        const [red, green, blue] = spectrogramColor(value);
        // Row zero is the lowest frequency and belongs at the bottom of the picture
        const y = spec.rows - 1 - row;
        const offset = (y * spec.cols + col) * 4;
        image.data[offset] = red;
        image.data[offset + 1] = green;
        image.data[offset + 2] = blue;
        image.data[offset + 3] = 255;
      }
    }
    context.putImageData(image, 0, 0);
  }, [state]);

  if (state.kind === "too-long") {
    return (
      <p className="small muted">
        {t("files.spectrogramTooLong", { minutes: Math.round(MAX_SPECTROGRAM_SECS / 60) })}
      </p>
    );
  }
  if (state.kind === "error") {
    return (
      <p className="small muted">
        {t("files.spectrogramError", { reason: state.reason })}
      </p>
    );
  }
  if (state.kind === "loading" || state.kind === "waiting") {
    return <p className="small muted">{t("files.spectrogramLoading")}</p>;
  }

  const percent =
    durationMs > 0 ? Math.max(0, Math.min(100, (currentMs / durationMs) * 100)) : 0;

  return (
    <div className="spectrogram">
      <canvas
        ref={canvasRef}
        className="spectrogram-canvas"
        role="img"
        aria-label={t("files.spectrogramLabel", {
          min: state.spec.minHz,
          max: state.spec.maxHz,
        })}
        onMouseDown={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          if (rect.width <= 0) {
            return;
          }
          const ratio = Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width));
          onSeek(Math.round(ratio * durationMs));
        }}
      />
      <span
        className="spectrogram-cursor"
        style={{ left: `${percent}%` }}
        aria-hidden="true"
      />
      <span className="spectrogram-scale mono" aria-hidden="true">
        {state.spec.minHz}–{state.spec.maxHz} Hz
      </span>
    </div>
  );
}
