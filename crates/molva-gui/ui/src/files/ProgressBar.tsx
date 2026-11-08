// SPDX-License-Identifier: MIT
// A bar that never invents a share of the work.
//
// `percent === null` renders an indeterminate bar: the browser moves the stripe, no number is
// shown, and assistive technology hears the phase as the value text instead of an invented
// percentage. A known percentage fills the track and nothing else.

export interface ProgressBarProps {
  percent: number | null;
  /** What the bar stands for, read by assistive technology */
  label: string;
  tone?: "running" | "done";
}

export default function ProgressBar({ percent, label, tone = "running" }: ProgressBarProps) {
  const known = percent !== null && Number.isFinite(percent);
  const value = known ? Math.max(0, Math.min(100, Math.round(percent))) : null;
  return (
    <div
      className={`progress ${tone}${value === null ? " indeterminate" : ""}`}
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value ?? undefined}
      aria-valuetext={label}
      aria-label={label}
    >
      <div
        className="progress-fill"
        style={value === null ? undefined : { width: `${value}%` }}
      />
    </div>
  );
}
