// SPDX-License-Identifier: MIT
// The loudness of one recording over time, drawn as a row of bars.
//
// The peaks come from Rust, one byte per bucket: the webview never decodes the file, which is what
// used to hang the window on a long recording. The picture is drawn on a canvas instead of one
// element per bar, so a wide window costs the same as a narrow one: the bar width is fixed and
// the canvas is redrawn at the size it is shown, whatever the window does.
//
// A press moves the player to the moment under the pointer, so the picture is the seek bar too.

import { useEffect, useRef, useState } from "react";

import { api, asCommandError } from "../api";
import { useI18n } from "../i18n";

/** A bar is this wide and the gap between two is this much, in CSS pixels */
const BAR_WIDTH = 2;
const BAR_GAP = 1;

/** The picture is this tall when the layout gives no height, in CSS pixels */
const FALLBACK_HEIGHT = 56;

type State =
  | { kind: "loading" }
  | { kind: "ready"; peaks: number[]; durationMs: number }
  | { kind: "error"; reason: string };

export interface AudioWaveformProps {
  path: string;
  /** The position the player reports, for the playhead */
  currentMs: number;
  onSeek: (ms: number) => void;
}

export default function AudioWaveform({ path, currentMs, onSeek }: AudioWaveformProps) {
  const { t } = useI18n();
  const [state, setState] = useState<State>({ kind: "loading" });
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setState({ kind: "loading" });
    api
      .waveform(path)
      .then((picture) => {
        if (!cancelled) {
          setState({ kind: "ready", peaks: picture.peaks, durationMs: picture.duration_ms });
        }
      })
      .catch((reason: unknown) => {
        if (!cancelled) {
          setState({ kind: "error", reason: asCommandError(reason).message });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  // The canvas follows the box it is in: a maximized window redraws the same picture wider
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    const observer = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect;
      if (box) {
        setWidth(box.width);
      }
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [state.kind]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || state.kind !== "ready" || width <= 0) {
      return;
    }
    const ratio = window.devicePixelRatio || 1;
    const height = canvas.clientHeight || FALLBACK_HEIGHT;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    const context = canvas.getContext("2d");
    if (!context) {
      return;
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);

    const styles = getComputedStyle(canvas);
    const idle = styles.getPropertyValue("--waveform-idle").trim() || "#c9a0b8";
    const played = styles.getPropertyValue("--waveform-played").trim() || "#b03a72";

    const step = BAR_WIDTH + BAR_GAP;
    const bars = Math.max(1, Math.floor(width / step));
    const playhead =
      state.durationMs > 0 ? (currentMs / state.durationMs) * bars : 0;
    for (let bar = 0; bar < bars; bar += 1) {
      const peak = state.peaks[Math.min(state.peaks.length - 1, Math.floor((bar / bars) * state.peaks.length))] ?? 0;
      const barHeight = Math.max(1, Math.round((peak / 255) * (height - 2)));
      context.fillStyle = bar <= playhead ? played : idle;
      context.fillRect(bar * step, height - barHeight, BAR_WIDTH, barHeight);
    }
  }, [state, width, currentMs]);

  if (state.kind === "loading") {
    return <p className="small muted">{t("files.mediaLoading")}</p>;
  }
  if (state.kind === "error") {
    return <p className="small muted">{t("files.waveformError", { reason: state.reason })}</p>;
  }

  const seekAt = (event: React.MouseEvent<HTMLCanvasElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const share = box.width > 0 ? (event.clientX - box.left) / box.width : 0;
    onSeek(Math.round(Math.min(1, Math.max(0, share)) * state.durationMs));
  };

  return (
    <canvas
      ref={canvasRef}
      className="waveform"
      role="slider"
      aria-label={t("files.waveform")}
      aria-valuemin={0}
      aria-valuemax={state.durationMs}
      aria-valuenow={Math.round(currentMs)}
      onClick={seekAt}
    />
  );
}
