// SPDX-License-Identifier: MIT
// Tests of the spectrogram: a tone lands in the band of its own frequency, silence stays dark,
// and the picture is bounded whatever the length of the file

import { describe, expect, it } from "vitest";

import {
  computeSpectrogram,
  monoMix,
  peakRow,
  rowFrequency,
} from "./spectrogram";

const RATE = 16_000;

function tone(hz: number, secs: number, amplitude = 0.8): Float32Array {
  const samples = new Float32Array(Math.round(RATE * secs));
  for (let i = 0; i < samples.length; i += 1) {
    samples[i] = amplitude * Math.sin((2 * Math.PI * hz * i) / RATE);
  }
  return samples;
}

describe("monoMix", () => {
  it("averages the channels", () => {
    const left = Float32Array.from([1, 0, 1, 0]);
    const right = Float32Array.from([0, 1, 0, 1]);
    const mixed = monoMix({
      numberOfChannels: 2,
      length: 4,
      getChannelData: (channel) => (channel === 0 ? left : right),
    });
    expect(Array.from(mixed)).toEqual([0.5, 0.5, 0.5, 0.5]);
  });

  it("returns a single channel untouched instead of copying it", () => {
    const samples = Float32Array.from([0.1, 0.2]);
    const mixed = monoMix({
      numberOfChannels: 1,
      length: 2,
      getChannelData: () => samples,
    });
    expect(mixed).toBe(samples);
  });
});

describe("computeSpectrogram", () => {
  it("puts a one-kilohertz tone into the band around one kilohertz", () => {
    const spec = computeSpectrogram(tone(1000, 1), RATE, {
      cols: 60,
      rows: 120,
      fftSize: 2048,
    });
    expect(spec.cols).toBe(60);
    expect(spec.rows).toBe(120);
    expect(spec.values).toHaveLength(60 * 120);
    const row = peakRow(spec, 30);
    const hz = rowFrequency(row, spec.rows, spec.minHz, spec.maxHz);
    expect(hz).toBeGreaterThan(800);
    expect(hz).toBeLessThan(1250);
  });

  it("separates two tones of different pitch", () => {
    const low = computeSpectrogram(tone(300, 0.5), RATE, {
      cols: 10,
      rows: 80,
      fftSize: 2048,
    });
    const high = computeSpectrogram(tone(3000, 0.5), RATE, {
      cols: 10,
      rows: 80,
      fftSize: 2048,
    });
    const lowHz = rowFrequency(
      peakRow(low, 5),
      low.rows,
      low.minHz,
      low.maxHz,
    );
    const highHz = rowFrequency(
      peakRow(high, 5),
      high.rows,
      high.minHz,
      high.maxHz,
    );
    expect(lowHz).toBeLessThan(highHz);
  });

  it("keeps silence dark instead of dividing by an empty maximum", () => {
    const spec = computeSpectrogram(new Float32Array(RATE), RATE, {
      cols: 20,
      rows: 40,
    });
    expect(spec.values.every((value) => value === 0)).toBe(true);
  });

  it("gives an empty picture for an empty signal", () => {
    const spec = computeSpectrogram(new Float32Array(0), RATE);
    expect(spec.cols).toBe(0);
    expect(spec.rows).toBe(0);
    expect(spec.values).toHaveLength(0);
  });

  it("covers the whole file with the requested number of columns", () => {
    const spec = computeSpectrogram(tone(1000, 2), RATE, {
      cols: 32,
      rows: 16,
    });
    expect(spec.cols).toBe(32);
    expect(spec.durationMs).toBeCloseTo(2000, -2);
  });

  it("raises a window that is not a power of two to the next one", () => {
    // A length of 1000 would break a radix-2 FFT; the picture must still appear
    const spec = computeSpectrogram(tone(500, 0.5), RATE, {
      cols: 8,
      rows: 24,
      fftSize: 1000,
    });
    expect(spec.values).toHaveLength(8 * 24);
  });

  it("keeps every value inside the byte range", () => {
    const spec = computeSpectrogram(tone(440, 0.3, 4), RATE, {
      cols: 9,
      rows: 9,
    });
    expect(spec.values.every((value) => value >= 0 && value <= 255)).toBe(true);
  });
});
