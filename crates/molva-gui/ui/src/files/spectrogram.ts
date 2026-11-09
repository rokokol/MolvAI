// SPDX-License-Identifier: MIT
// A spectrogram of a whole audio file, computed for a picture the width of a card.
//
// The picture is bounded in both directions: `cols` columns cover the file from its first to
// its last sample, whatever its length, and `rows` bands cover 80 Hz to 8 kHz on a logarithmic
// scale, which is where speech lives. Time resolution therefore falls on a long file, while
// memory stays at `cols * rows` bytes. The window is a plain radix-2 FFT: the interface needs no
// library for one picture.

/** One spectrogram picture */
export interface Spectrogram {
  cols: number;
  rows: number;
  /** Row-major, row 0 is the lowest frequency; 0 is silence, 255 the loudest band */
  values: Uint8Array;
  durationMs: number;
  minHz: number;
  maxHz: number;
}

export interface SpectrogramOptions {
  cols?: number;
  rows?: number;
  fftSize?: number;
  minHz?: number;
  maxHz?: number;
  /** Dynamic range below the loudest band, in decibels */
  rangeDb?: number;
}

const DEFAULTS = {
  cols: 900,
  rows: 120,
  fftSize: 2048,
  minHz: 80,
  maxHz: 8000,
  rangeDb: 70,
};

/** Decoded audio, or samples of one channel. A long stereo file needs no mixed copy */
export type SpectrogramInput =
  | Float32Array
  | {
      numberOfChannels: number;
      length: number;
      getChannelData: (channel: number) => Float32Array;
    };

/** Channel arrays of the input; the downmix happens inside each FFT window */
function channelsOf(input: SpectrogramInput): Float32Array[] {
  if (input instanceof Float32Array) {
    return [input];
  }
  return Array.from({ length: Math.max(1, input.numberOfChannels) }, (_, channel) =>
    input.getChannelData(channel),
  );
}

/** In-place iterative radix-2 FFT; the length must be a power of two */
function fft(real: Float64Array, imag: Float64Array): void {
  const n = real.length;
  for (let i = 1, j = 0; i < n; i += 1) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) {
      j ^= bit;
    }
    j ^= bit;
    if (i < j) {
      [real[i], real[j]] = [real[j], real[i]];
      [imag[i], imag[j]] = [imag[j], imag[i]];
    }
  }
  for (let length = 2; length <= n; length <<= 1) {
    const angle = (-2 * Math.PI) / length;
    const stepReal = Math.cos(angle);
    const stepImag = Math.sin(angle);
    for (let start = 0; start < n; start += length) {
      let wReal = 1;
      let wImag = 0;
      for (let offset = 0; offset < length / 2; offset += 1) {
        const even = start + offset;
        const odd = even + length / 2;
        const oddReal = real[odd] * wReal - imag[odd] * wImag;
        const oddImag = real[odd] * wImag + imag[odd] * wReal;
        real[odd] = real[even] - oddReal;
        imag[odd] = imag[even] - oddImag;
        real[even] += oddReal;
        imag[even] += oddImag;
        const nextReal = wReal * stepReal - wImag * stepImag;
        wImag = wReal * stepImag + wImag * stepReal;
        wReal = nextReal;
      }
    }
  }
}

/** The smallest power of two that is not below `value`, so the FFT stays radix-2 */
function nextPowerOfTwo(value: number): number {
  let size = 2;
  while (size < value) {
    size <<= 1;
  }
  return size;
}

/** The bin ranges of the logarithmic rows: row 0 is the lowest band */
function rowBands(
  rows: number,
  fftSize: number,
  sampleRate: number,
  minHz: number,
  maxHz: number,
): { from: number; to: number }[] {
  const binHz = sampleRate / fftSize;
  const nyquistBin = Math.floor(fftSize / 2);
  const frequency = (row: number) => {
    if (rows <= 1) {
      return maxHz;
    }
    return minHz * (maxHz / minHz) ** (row / (rows - 1));
  };
  const bands: { from: number; to: number }[] = [];
  for (let row = 0; row < rows; row += 1) {
    const low = row === 0 ? minHz : frequency(row - 0.5);
    const high = row === rows - 1 ? maxHz : frequency(row + 0.5);
    const from = Math.max(0, Math.min(nyquistBin - 1, Math.floor(low / binHz)));
    const to = Math.max(from, Math.min(nyquistBin, Math.ceil(high / binHz)));
    bands.push({ from, to });
  }
  return bands;
}

/**
 * Compute the picture
 *
 * An empty signal gives an empty picture instead of a division by zero. The values are
 * normalized per file: the loudest band of the file is 255, `rangeDb` below it is 0.
 */
export function computeSpectrogram(
  input: SpectrogramInput,
  sampleRate: number,
  options: SpectrogramOptions = {},
): Spectrogram {
  const channels = channelsOf(input);
  const length = channels[0]?.length ?? 0;
  const cols = Math.max(1, Math.floor(options.cols ?? DEFAULTS.cols));
  const rows = Math.max(1, Math.floor(options.rows ?? DEFAULTS.rows));
  const fftSize = nextPowerOfTwo(
    Math.max(2, Math.floor(options.fftSize ?? DEFAULTS.fftSize)),
  );
  const { minHz, maxHz, rangeDb } = { ...DEFAULTS, ...options };
  const empty: Spectrogram = {
    cols: 0,
    rows: 0,
    values: new Uint8Array(0),
    durationMs: 0,
    minHz,
    maxHz,
  };
  if (length === 0 || !Number.isFinite(sampleRate) || sampleRate <= 0) {
    return empty;
  }
  const window = new Float64Array(fftSize);
  for (let i = 0; i < fftSize; i += 1) {
    window[i] = 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / (fftSize - 1));
  }
  const bands = rowBands(rows, fftSize, sampleRate, minHz, maxHz);
  const real = new Float64Array(fftSize);
  const imag = new Float64Array(fftSize);
  const magnitudes = new Float32Array(cols * rows);
  const span = length - fftSize;
  const hop = cols > 1 ? Math.max(1, Math.floor(span / (cols - 1))) : 1;
  const gain = 1 / channels.length;
  for (let col = 0; col < cols; col += 1) {
    const start =
      span <= 0 ? 0 : Math.min(span, Math.max(0, Math.round(col * hop)));
    for (let i = 0; i < fftSize; i += 1) {
      const index = start + i;
      let sample = 0;
      if (index < length) {
        for (let channel = 0; channel < channels.length; channel += 1) {
          sample += channels[channel][index] * gain;
        }
      }
      real[i] = sample * window[i];
      imag[i] = 0;
    }
    fft(real, imag);
    for (let row = 0; row < rows; row += 1) {
      const band = bands[row];
      let peak = 0;
      for (let bin = band.from; bin < band.to; bin += 1) {
        const power = real[bin] * real[bin] + imag[bin] * imag[bin];
        if (power > peak) {
          peak = power;
        }
      }
      magnitudes[col * rows + row] = Math.sqrt(peak);
    }
  }
  let maxMagnitude = 0;
  for (let index = 0; index < magnitudes.length; index += 1) {
    if (magnitudes[index] > maxMagnitude) {
      maxMagnitude = magnitudes[index];
    }
  }
  const values = new Uint8Array(cols * rows);
  // Silence has no loudest band to normalize against: the picture is dark, not full
  if (maxMagnitude <= 1e-9) {
    return {
      cols,
      rows,
      values,
      durationMs: (length / sampleRate) * 1000,
      minHz,
      maxHz,
    };
  }
  const maxDb = 20 * Math.log10(maxMagnitude);
  const floor = maxDb - rangeDb;
  for (let index = 0; index < magnitudes.length; index += 1) {
    const db = 20 * Math.log10(magnitudes[index] + 1e-12);
    const level = (db - floor) / rangeDb;
    values[index] = Math.max(0, Math.min(255, Math.round(level * 255)));
  }
  return {
    cols,
    rows,
    values,
    durationMs: (length / sampleRate) * 1000,
    minHz,
    maxHz,
  };
}

/** The frequency of a row, for the readout above the picture */
export function rowFrequency(
  row: number,
  rows: number,
  minHz: number,
  maxHz: number,
): number {
  if (rows <= 1) {
    return maxHz;
  }
  return minHz * (maxHz / minHz) ** (row / (rows - 1));
}

/** The loudest row of one column, as a frequency */
export function peakRow(spec: Spectrogram, col: number): number {
  let best = 0;
  let bestRow = 0;
  for (let row = 0; row < spec.rows; row += 1) {
    const value = spec.values[col * spec.rows + row];
    if (value > best) {
      best = value;
      bestRow = row;
    }
  }
  return bestRow;
}

/** Colour stops from silence to the loudest band; the ramp is monotonic in luminance */
const COLOUR_STOPS: { at: number; rgb: [number, number, number] }[] = [
  { at: 0.0, rgb: [9, 11, 20] },
  { at: 0.3, rgb: [39, 67, 140] },
  { at: 0.65, rgb: [214, 120, 60] },
  { at: 1.0, rgb: [255, 238, 190] },
];

/** The colour of one spectrogram value */
export function spectrogramColor(value: number): [number, number, number] {
  const level = Math.max(0, Math.min(255, value)) / 255;
  for (let index = 1; index < COLOUR_STOPS.length; index += 1) {
    const high = COLOUR_STOPS[index];
    if (level <= high.at) {
      const low = COLOUR_STOPS[index - 1];
      const span = high.at - low.at;
      const ratio = span <= 0 ? 0 : (level - low.at) / span;
      return [
        Math.round(low.rgb[0] + (high.rgb[0] - low.rgb[0]) * ratio),
        Math.round(low.rgb[1] + (high.rgb[1] - low.rgb[1]) * ratio),
        Math.round(low.rgb[2] + (high.rgb[2] - low.rgb[2]) * ratio),
      ];
    }
  }
  return COLOUR_STOPS[COLOUR_STOPS.length - 1].rgb;
}
