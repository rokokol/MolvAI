// SPDX-License-Identifier: MIT
// The key of a cloud service: where it comes from, the pasted value and the entry that holds it
//
// The post-processing model and the remote recognizer ask the same questions of the same store,
// so they share one field. Only the entry name differs: a model keeps one per provider, a
// recognizer keeps one for the single provider it is configured with

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError } from "../api";
import { useI18n } from "../i18n";
import type { CommandError, ProviderKeyStatus } from "../types";
import {
  keyField,
  keySourceChoices,
  storeUnavailable,
  storesInKeyring,
  warnsAboutClearText,
} from "./keysource";

export interface ProviderKeyFieldProps {
  /** Which service the entry belongs to: it decides the command and the entry name */
  kind: "llm" | "stt";
  /** Prefix of the field ids, so both sections can sit on one page */
  idPrefix: string;
  /** The provider of the model; ignored by the recognizer, which has one */
  provider?: string;
  source: string;
  onSource: (source: string) => void;
  /** The sources this service accepts; the recognizer keeps no key in clear text */
  sources?: readonly string[];
  env: string;
  onEnv: (env: string) => void;
  plain?: string;
  onPlain?: (plain: string) => void;
  onError: (error: CommandError | null) => void;
}

export default function ProviderKeyField({
  kind,
  idPrefix,
  provider = "",
  source,
  onSource,
  sources,
  env,
  onEnv,
  plain = "",
  onPlain = () => undefined,
  onError,
}: ProviderKeyFieldProps) {
  const { t } = useI18n();
  const [status, setStatus] = useState<ProviderKeyStatus | null>(null);
  const [pasted, setPasted] = useState("");
  const [busy, setBusy] = useState(false);

  const loadStatus = useCallback(
    (wanted: string): Promise<ProviderKeyStatus> =>
      kind === "llm"
        ? api.providerKeyStatus(provider, wanted)
        : api.sttKeyStatus(wanted),
    [kind, provider],
  );
  const storeEntry = useCallback(
    (wanted: string, key: string): Promise<ProviderKeyStatus> =>
      kind === "llm"
        ? api.storeProviderKey(provider, wanted, key)
        : api.storeSttKey(wanted, key),
    [kind, provider],
  );
  const removeEntry = useCallback(
    (wanted: string): Promise<ProviderKeyStatus> =>
      kind === "llm"
        ? api.removeProviderKey(provider, wanted)
        : api.removeSttKey(wanted),
    [kind, provider],
  );

  // Whether the entry exists is a property of the store, not of the form: a keystroke elsewhere
  // must not ask the store again
  useEffect(() => {
    if (!storesInKeyring(source)) {
      setStatus(null);
      return;
    }
    loadStatus(source)
      .then(setStatus)
      .catch((err) =>
        setStatus({ present: false, entry: "", detail: asCommandError(err).message }),
      );
  }, [source, loadStatus]);

  const store = async () => {
    setBusy(true);
    try {
      setStatus(await storeEntry(source, pasted));
      // The value leaves the page at once: it lives in the store, not in the window
      setPasted("");
    } catch (err) {
      onError(asCommandError(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      setStatus(await removeEntry(source));
    } catch (err) {
      onError(asCommandError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="field">
        <label htmlFor={`${idPrefix}-key-source`}>{t("settings.llm.keySource")}</label>
        <select
          id={`${idPrefix}-key-source`}
          value={source}
          onChange={(event) => onSource(event.target.value)}
        >
          {keySourceChoices(source, sources).map((choice) => (
            <option key={choice} value={choice}>
              {t(`settings.keySource.${keyField(choice)}`, { source: choice })}
            </option>
          ))}
        </select>
        <p className="small muted">{t(`settings.keySourceHint.${keyField(source)}`)}</p>
      </div>

      {keyField(source) === "store" && (
        <div className="field wide">
          <label htmlFor={`${idPrefix}-key-value`}>{t("settings.llm.keyValue")}</label>
          <div className="field-inline">
            <input
              id={`${idPrefix}-key-value`}
              type="password"
              autoComplete="off"
              value={pasted}
              onChange={(event) => setPasted(event.target.value)}
            />
            <button
              type="button"
              className="primary"
              disabled={busy || pasted.trim() === ""}
              onClick={() => void store()}
            >
              {t("settings.llm.keyStore")}
            </button>
            <button
              type="button"
              disabled={busy || status?.present !== true}
              onClick={() => void remove()}
            >
              {t("settings.llm.keyRemove")}
            </button>
          </div>
          <p className="small" aria-live="polite">
            {status?.present
              ? t("settings.llm.keyPresent", { entry: status.entry })
              : t("settings.llm.keyMissing")}
          </p>
          {status !== null && storeUnavailable(status) && status.detail && (
            <div className="notice warning" role="alert">
              <p>{t("settings.llm.keyStoreRefused")}</p>
              <p className="mono small">{status.detail}</p>
            </div>
          )}
        </div>
      )}

      {warnsAboutClearText(source) && (
        <>
          <div className="notice warning" role="alert">
            <p>{t("settings.keySource.plainWarning")}</p>
          </div>
          <div className="field wide">
            <label htmlFor={`${idPrefix}-key-plain`}>{t("settings.llm.keyValue")}</label>
            <input
              id={`${idPrefix}-key-plain`}
              type="password"
              autoComplete="off"
              value={plain}
              onChange={(event) => onPlain(event.target.value)}
            />
          </div>
        </>
      )}

      {keyField(source) === "env" && (
        <div className="field">
          <label htmlFor={`${idPrefix}-key-env`}>{t("settings.llm.keyEnv")}</label>
          <input
            id={`${idPrefix}-key-env`}
            type="text"
            value={env}
            onChange={(event) => onEnv(event.target.value)}
          />
        </div>
      )}
    </>
  );
}
