// SPDX-License-Identifier: MIT
// The custom style list: one entry is open at a time. A custom entry with the same id takes a
// built-in place, which is what the core does with it

import { useState } from "react";

import { useI18n } from "../i18n";
import type { CustomStyle, StyleOption } from "../types";
import {
  canAddStyle,
  issueFor,
  MAX_CUSTOM_STYLES,
  newCustomStyle,
  overridesBuiltin,
  styleIssues,
} from "./styles";

interface Props {
  styles: CustomStyle[];
  builtins: StyleOption[];
  onChange: (styles: CustomStyle[]) => void;
}

export function StyleEditor({ styles, builtins, onChange }: Props) {
  const { t } = useI18n();
  const [selected, setSelected] = useState(0);
  const issues = styleIssues(styles);
  const index = styles.length === 0 ? 0 : Math.min(selected, styles.length - 1);
  const style = styles[index];

  const change = (patch: Partial<CustomStyle>) => {
    onChange(
      styles.map((item, at) =>
        at === index ? { ...item, uses_llm: true, ...patch } : item,
      ),
    );
  };

  return (
    <div className="style-editor">
      <h4>
        {t("settings.styles.custom", {
          count: styles.length,
          max: MAX_CUSTOM_STYLES,
        })}
      </h4>
      <p className="small muted">{t("settings.styles.customHint")}</p>

      <div className="style-picker">
        <label htmlFor="style-picker">{t("settings.styles.pick")}</label>
        <select
          id="style-picker"
          value={style ? String(index) : ""}
          disabled={styles.length === 0}
          onChange={(event) => setSelected(Number(event.target.value))}
        >
          {styles.map((item, at) => (
            <option key={`${item.id}-${at}`} value={at}>
              {item.name.trim() === "" ? t("settings.styles.untitled") : item.name}
            </option>
          ))}
        </select>
        <button
          type="button"
          disabled={!canAddStyle(styles)}
          onClick={() => {
            onChange([...styles, newCustomStyle(styles)]);
            setSelected(styles.length);
          }}
        >
          {t("settings.styles.add")}
        </button>
      </div>

      {style && (
        <fieldset className="style-entry">
          <legend>
            {style.name.trim() === "" ? t("settings.styles.untitled") : style.name}
            {overridesBuiltin(style.id, builtins) && (
              <span className="badge">{t("settings.styles.overrides")}</span>
            )}
          </legend>
          <div className="field">
            <label htmlFor="style-id">{t("settings.styles.id")}</label>
            <input
              id="style-id"
              type="text"
              value={style.id}
              onChange={(event) => change({ id: event.target.value })}
            />
            {issueFor(issues, index, "id") && (
              <p className="small error-text" role="alert">
                {t(issueFor(issues, index, "id")?.key ?? "")}
              </p>
            )}
          </div>
          <div className="field">
            <label htmlFor="style-name">{t("settings.styles.name")}</label>
            <input
              id="style-name"
              type="text"
              value={style.name}
              onChange={(event) => change({ name: event.target.value })}
            />
            {issueFor(issues, index, "name") && (
              <p className="small error-text" role="alert">
                {t(issueFor(issues, index, "name")?.key ?? "")}
              </p>
            )}
          </div>
          <div className="field wide">
            <label htmlFor="style-prompt">{t("settings.styles.prompt")}</label>
            <textarea
              id="style-prompt"
              value={style.system_prompt}
              onChange={(event) => change({ system_prompt: event.target.value })}
            />
            {issueFor(issues, index, "system_prompt") && (
              <p className="small error-text" role="alert">
                {t(issueFor(issues, index, "system_prompt")?.key ?? "")}
              </p>
            )}
          </div>
          <button
            type="button"
            className="danger chip"
            onClick={() => {
              onChange(styles.filter((_item, at) => at !== index));
              setSelected(Math.max(0, index - 1));
            }}
          >
            {t("settings.styles.remove")}
          </button>
        </fieldset>
      )}

      <details className="help">
        <summary>{t("settings.styles.builtin")}</summary>
        <p className="small muted">{t("settings.styles.builtinHint")}</p>
        <ul className="plain-list">
          {builtins.map((item) => (
            <li key={item.id}>
              <span>{item.name}</span>
              <code className="mono">{item.id}</code>
            </li>
          ))}
        </ul>
      </details>
    </div>
  );
}
