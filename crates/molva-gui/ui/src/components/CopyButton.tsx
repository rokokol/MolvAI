// SPDX-License-Identifier: MIT
// One copy control for terminal rows: the mark, the check and the alert come from the ddlc
// stylesheet; this component only carries the state and the accessible label

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
      data-state={copied ? "copied" : undefined}
      onClick={onClick}
    >
      <span className="sr-only">{copied ? copiedLabel : label}</span>
    </button>
  );
}
