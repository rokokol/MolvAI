// SPDX-License-Identifier: MIT
// Snippets: phrases that return a prepared answer before the model runs
//
// The editor keeps a draft of the config: adding, editing or removing a snippet changes only
// this page until Save writes the whole file. The page can filter its cards by phrase or
// replacement; a managed config stays read-only, because the Nix store is the source of truth

import { useEffect, useState } from "react";

import { api, asCommandError } from "../api";
import { useI18n } from "../i18n";
import type { Snippet } from "../types";
import type { ViewProps } from "../App";

interface Props extends ViewProps {
  /** The config comes from the Nix store: the page is visible only for inspection */
  managed?: boolean;
}

const MAX_SNIPPETS = 100;

export default function Snippets({ config, reloadConfig, onError, managed = false }: Props) {
  const { t } = useI18n();
  const [draft, setDraft] = useState(config);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");

  useEffect(() => {
    setDraft(config);
  }, [config]);

  if (!draft) {
    return (
      <>
        <h1>{t("snippets.title")}</h1>
        <p className="empty">{t("common.loading")}</p>
      </>
    );
  }

  const { enabled, items } = draft.snippets;
  const needle = query.trim().toLowerCase();
  const visible = items
    .map((item, index) => ({ item, index }))
    .filter(
      ({ item }) =>
        needle === "" ||
        item.phrase.toLowerCase().includes(needle) ||
        item.replacement.toLowerCase().includes(needle),
    );

  const change = (next: { enabled?: boolean; items?: Snippet[] }) => {
    setDraft({
      ...draft,
      snippets: {
        enabled: next.enabled ?? enabled,
        items: next.items ?? items,
      },
    });
    setSaved(false);
  };

  const replace = (index: number, patch: Partial<Snippet>) => {
    change({
      items: items.map((item, at) => (at === index ? { ...item, ...patch } : item)),
    });
  };

  const save = async () => {
    setBusy(true);
    try {
      await api.saveConfig({ ...draft, snippets: { enabled, items } });
      setSaved(true);
      await reloadConfig();
    } catch (err) {
      onError(asCommandError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <h1>{t("snippets.title")}</h1>

      <section className="card">
        <div className="row spread">
          <h2>{t("snippets.enabled")}</h2>
          <label className="check">
            <input
              type="checkbox"
              checked={enabled}
              disabled={managed}
              onChange={(event) => change({ enabled: event.target.checked })}
            />
            {t("snippets.enabledCheck")}
          </label>
        </div>
        <p className="small muted">{t("snippets.enabledHint")}</p>

        {items.length > 0 && (
          <div className="field wide">
            <label htmlFor="snippet-search">{t("snippets.search")}</label>
            <input
              id="snippet-search"
              type="search"
              value={query}
              placeholder={t("snippets.searchPlaceholder")}
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        )}

        {items.length === 0 ? (
          <p className="empty">{t("snippets.empty")}</p>
        ) : visible.length === 0 ? (
          <p className="empty">
            {t("snippets.noMatches", { count: items.length })}
          </p>
        ) : (
          visible.map(({ item, index }) => (
            <fieldset className="style-entry" key={index}>
              <legend>
                {t("snippets.entry", { number: index + 1 })}
              </legend>
              <div className="field wide">
                <label htmlFor={`snippet-phrase-${index}`}>{t("snippets.phrase")}</label>
                <input
                  id={`snippet-phrase-${index}`}
                  type="text"
                  value={item.phrase}
                  disabled={managed}
                  placeholder={t("snippets.phrasePlaceholder")}
                  onChange={(event) => replace(index, { phrase: event.target.value })}
                />
              </div>
              <div className="field wide">
                <label htmlFor={`snippet-replacement-${index}`}>
                  {t("snippets.replacement")}
                </label>
                <textarea
                  id={`snippet-replacement-${index}`}
                  value={item.replacement}
                  disabled={managed}
                  rows={4}
                  placeholder={t("snippets.replacementPlaceholder")}
                  onChange={(event) =>
                    replace(index, { replacement: event.target.value })
                  }
                />
              </div>
              <button
                type="button"
                className="danger chip"
                disabled={managed}
                onClick={() => change({ items: items.filter((_item, at) => at !== index) })}
              >
                {t("snippets.remove")}
              </button>
            </fieldset>
          ))
        )}

        <div className="row">
          <button
            type="button"
            disabled={managed || items.length >= MAX_SNIPPETS}
            onClick={() => change({ items: [...items, { phrase: "", replacement: "" }] })}
          >
            {t("snippets.add")}
          </button>
          <button
            type="button"
            className="primary"
            disabled={managed || busy}
            onClick={() => void save()}
          >
            {busy ? t("snippets.saving") : t("common.save")}
          </button>
          {saved && <span className="small muted">{t("settings.saved")}</span>}
        </div>
        {managed && <p className="small muted">{t("snippets.managed")}</p>}
      </section>

      <section className="card">
        <h2>{t("snippets.howTitle")}</h2>
        <p>{t("snippets.howBody")}</p>
        <p className="small muted">{t("snippets.howHint")}</p>
      </section>
    </>
  );
}
