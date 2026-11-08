// SPDX-License-Identifier: MIT
// One copy control for terminal rows: the icon is the visible face, and the label stays in
// the accessibility tree. The check mark replaces the mark after a successful copy

interface Props {
  copied: boolean;
  label: string;
  copiedLabel: string;
  onClick: () => void;
}

export default function CopyButton({ copied, label, copiedLabel, onClick }: Props) {
  return (
    <button
      type="button"
      className="chip copy-button"
      aria-label={copied ? copiedLabel : label}
      title={copied ? copiedLabel : label}
      onClick={onClick}
    >
      <svg viewBox="0 0 12 12" aria-hidden="true">
        {copied ? (
          <path d="M2 6.5 4.6 9 10 3" fill="none" stroke="currentColor" strokeWidth="1.6" />
        ) : (
          <>
            <rect
              x="4"
              y="1.5"
              width="6.5"
              height="7.5"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.4"
            />
            <rect
              x="1.5"
              y="3.5"
              width="6.5"
              height="7"
              fill="var(--ddlc-ground)"
              stroke="currentColor"
              strokeWidth="1.4"
            />
          </>
        )}
      </svg>
      <span className="sr-only">{copied ? copiedLabel : label}</span>
    </button>
  );
}
