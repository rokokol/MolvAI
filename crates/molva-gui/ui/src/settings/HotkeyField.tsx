// SPDX-License-Identifier: MIT
// A hotkey is recorded, not typed: the field opens a small dialog, the dialog reads the real
// key combination, and the value reaches the settings only after a confirmation

import { useCallback, useEffect, useRef, useState } from "react";

import { api, asCommandError } from "../api";
import { useI18n } from "../i18n";
import { captureHotkey, normalizeHotkey } from "./hotkey";

interface Props {
  id: string;
  label: string;
  /** The value of `hotkeys.*`: the text the daemon parses */
  value: string;
  onChange: (value: string) => void;
}

export default function HotkeyField({ id, label, value, onChange }: Props) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [captured, setCaptured] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const dialog = useRef<HTMLDivElement>(null);

  const close = useCallback(() => {
    setOpen(false);
    setCaptured(null);
    setHint(null);
    setError(null);
    setChecking(false);
  }, []);

  // The dialog takes the keyboard focus: a captured key would otherwise land in the field
  // behind it, and Escape would reach the page instead of the dialog
  useEffect(() => {
    if (open) {
      dialog.current?.focus();
    }
  }, [open]);

  const record = async (event: React.KeyboardEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      close();
      return;
    }
    const result = captureHotkey(event.nativeEvent);
    if (result.kind === "modifier") {
      setHint(t("settings.hotkeys.modifierOnly"));
      setCaptured(null);
      setError(null);
      return;
    }
    if (result.kind === "unsupported") {
      setError(t("settings.hotkeys.unsupported"));
      setCaptured(null);
      return;
    }
    setHint(null);
    setCaptured(result.value);
    setError(null);
    // The parser of the core decides: a combination it cannot bind never reaches the config
    setChecking(true);
    try {
      await api.checkHotkey(result.value);
      setError(null);
    } catch (err) {
      setError(asCommandError(err).message);
    } finally {
      setChecking(false);
    }
  };

  const shown = normalizeHotkey(value);

  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <button
        id={id}
        type="button"
        className="hotkey-value"
        onClick={() => setOpen(true)}
        aria-haspopup="dialog"
        aria-label={`${label}: ${t("settings.hotkeys.record")}`}
        title={t("settings.hotkeys.record")}
      >
        {shown === "" ? t("settings.hotkeys.unset") : <kbd>{shown}</kbd>}
        <span className="hotkey-record" aria-hidden="true">
          <svg viewBox="0 0 12 12" width="12" height="12" focusable="false">
            <circle cx="6" cy="6" r="4" fill="currentColor" />
          </svg>
        </span>
      </button>
      {open && (
        <div className="modal-backdrop" onMouseDown={close}>
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby={`${id}-title`}
            ref={dialog}
            tabIndex={-1}
            onMouseDown={(event) => event.stopPropagation()}
            onKeyDown={(event) => void record(event)}
          >
            <h3 id={`${id}-title`}>{label}</h3>
            <p className="muted small">{t("settings.hotkeys.prompt")}</p>
            <div className="modal-capture" aria-live="polite">
              {captured === null ? (
                <span className="muted">{t("settings.hotkeys.waiting")}</span>
              ) : (
                <kbd>{captured}</kbd>
              )}
            </div>
            {hint && <p className="small muted">{hint}</p>}
            {error && (
              <p className="small" role="alert">
                {error}
              </p>
            )}
            <div className="row modal-actions">
              <button type="button" onClick={close}>
                {t("common.cancel")}
              </button>
              <button
                type="button"
                className="primary"
                disabled={captured === null || checking || error !== null}
                onClick={() => {
                  if (captured !== null) {
                    onChange(captured);
                  }
                  close();
                }}
              >
                {t("common.confirm")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
