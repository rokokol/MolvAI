// SPDX-License-Identifier: MIT
// Настройки: форма поверх Config, валидация на стороне Rust, экспорт и импорт TOML.

import { useCallback, useEffect, useState, type ReactNode } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import { LANGUAGES, useI18n, type Lang } from "../i18n";
import type { Theme, ViewProps } from "../App";
import type { CommandError, Config, DeviceInfo, StyleOption } from "../types";

interface Props extends ViewProps {
  theme: Theme;
  onThemeChange: (theme: Theme) => void;
}

/** Пресеты провайдеров: выбор подставляет адрес, модель и имя переменной с ключом. */
const PROVIDERS: Record<string, { base_url: string; model: string; api_key_env: string }> = {
  ollama: { base_url: "http://localhost:11434/v1", model: "qwen3.5:4b", api_key_env: "" },
  lmstudio: { base_url: "http://localhost:1234/v1", model: "local-model", api_key_env: "" },
  openrouter: {
    base_url: "https://openrouter.ai/api/v1",
    model: "openai/gpt-4o-mini",
    api_key_env: "OPENROUTER_API_KEY",
  },
  groq: {
    base_url: "https://api.groq.com/openai/v1",
    model: "llama-3.3-70b-versatile",
    api_key_env: "GROQ_API_KEY",
  },
  openai: {
    base_url: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
    api_key_env: "OPENAI_API_KEY",
  },
  custom: { base_url: "", model: "", api_key_env: "" },
};

const CLOUD_PROVIDERS = ["openrouter", "groq", "openai"];
const MODELS = ["tiny", "base", "small", "medium", "large-v3-turbo"];
const OUTPUT_MODES = ["auto", "paste", "type", "clipboard"];
const THEMES: Theme[] = ["system", "light", "dark"];

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="card">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Check({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="check">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}

export default function Settings({
  config,
  reloadConfig,
  onError,
  theme,
  onThemeChange,
}: Props) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<Config | null>(config);
  const [saved, setSaved] = useState(false);
  const [invalid, setInvalid] = useState<CommandError | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [styles, setStyles] = useState<StyleOption[]>([]);
  const [autostart, setAutostart] = useState(false);
  const [configPath, setConfigPath] = useState("");
  const [snippet, setSnippet] = useState("");
  const [snippetCopied, setSnippetCopied] = useState(false);
  const [importPath, setImportPath] = useState("");
  const [exportedTo, setExportedTo] = useState("");
  const [cloudAccepted, setCloudAccepted] = useState(false);
  const [newLanguage, setNewLanguage] = useState("");

  useEffect(() => {
    setDraft(config);
  }, [config]);

  useEffect(() => {
    api.getAutostart().then(setAutostart).catch(() => setAutostart(false));
    api.getConfigPath().then(setConfigPath).catch(() => setConfigPath(""));
    api.availableStyles().then(setStyles).catch(() => setStyles([]));
    api.listDevices().then(setDevices).catch(() => setDevices([]));
  }, []);

  useEffect(() => {
    api.hyprlandSnippet().then(setSnippet).catch(() => setSnippet(""));
  }, [config]);

  const patch = useCallback((change: (next: Config) => void) => {
    setSaved(false);
    setDraft((prev) => {
      if (!prev) {
        return prev;
      }
      // Копия на один уровень глубже мутируемой ветки достаточна: change правит
      // вложенные объекты, поэтому клонируем целиком через structuredClone.
      const next = structuredClone(prev);
      change(next);
      return next;
    });
  }, []);

  if (!draft) {
    return (
      <>
        <h1>{t("settings.title")}</h1>
        <p className="muted">{t("common.loading")}</p>
      </>
    );
  }

  const cloudSelected = draft.llm.enabled && CLOUD_PROVIDERS.includes(draft.llm.provider);
  const blockedByCloud = cloudSelected && !cloudAccepted;

  const save = async () => {
    try {
      await api.saveConfig(draft);
      setInvalid(null);
      setSaved(true);
      onError(null);
      await reloadConfig();
    } catch (err) {
      const error = asCommandError(err);
      setInvalid(error);
      onError(error);
    }
  };

  const reset = async () => {
    if (!window.confirm(t("settings.confirmReset"))) {
      return;
    }
    try {
      setDraft(await api.resetConfig());
      await reloadConfig();
    } catch (err) {
      onError(asCommandError(err));
    }
  };

  return (
    <>
      <h1>{t("settings.title")}</h1>

      {invalid && (
        <div className="notice error" role="alert">
          <strong>{t("error.validation")}</strong>
          <p>{invalid.message}</p>
          {invalid.field && <p>{t("error.field", { field: invalid.field })}</p>}
          {invalid.hint && <p>{invalid.hint}</p>}
        </div>
      )}
      {saved && (
        <div className="notice ok" role="status">
          {t("settings.saved")}
        </div>
      )}

      <Section title={t("settings.section.general")}>
        <div className="grid2">
          <div className="field">
            <label htmlFor="ui-language">{t("settings.language")}</label>
            <select
              id="ui-language"
              value={draft.ui_language}
              onChange={(event) =>
                patch((next) => {
                  next.ui_language = event.target.value as Lang;
                })
              }
            >
              {LANGUAGES.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="ui-theme">{t("settings.theme")}</label>
            <select
              id="ui-theme"
              value={theme}
              onChange={(event) => onThemeChange(event.target.value as Theme)}
            >
              {THEMES.map((item) => (
                <option key={item} value={item}>
                  {t(`settings.theme.${item}`)}
                </option>
              ))}
            </select>
          </div>
        </div>
        <Check
          label={t("settings.autostart")}
          checked={autostart}
          onChange={(value) => {
            api
              .setAutostart(value)
              .then(setAutostart)
              .catch((err) => onError(asCommandError(err)));
          }}
        />
        {configPath && (
          <p className="small muted">
            {t("settings.configPath")}: <span className="mono">{configPath}</span>
          </p>
        )}

        <h3>{t("settings.hotkeys")}</h3>
        <div className="grid2">
          {(
            [
              ["push_to_talk", "settings.hotkeys.pushToTalk"],
              ["toggle", "settings.hotkeys.toggle"],
              ["command", "settings.hotkeys.command"],
              ["cancel", "settings.hotkeys.cancel"],
              ["style_next", "settings.hotkeys.styleNext"],
            ] as const
          ).map(([key, label]) => (
            <div className="field" key={key}>
              <label htmlFor={`hotkey-${key}`}>{t(label)}</label>
              <input
                id={`hotkey-${key}`}
                type="text"
                value={draft.hotkeys[key]}
                onChange={(event) =>
                  patch((next) => {
                    next.hotkeys[key] = event.target.value;
                  })
                }
              />
            </div>
          ))}
        </div>

        <div className="notice">
          <strong>{t("settings.wayland.title")}</strong>
          <p>{t("settings.wayland.body")}</p>
          <textarea readOnly rows={7} value={snippet} aria-label={t("settings.wayland.title")} />
          <div className="row" style={{ marginTop: "0.4rem" }}>
            <button
              type="button"
              className="chip"
              onClick={async () => setSnippetCopied(await copyToClipboard(snippet))}
            >
              {snippetCopied ? t("common.copied") : t("common.copy")}
            </button>
          </div>
        </div>
      </Section>

      <Section title={t("settings.section.audio")}>
        <div className="grid2">
          <div className="field">
            <label htmlFor="audio-device">{t("settings.device")}</label>
            <select
              id="audio-device"
              value={draft.audio.device}
              onChange={(event) =>
                patch((next) => {
                  next.audio.device = event.target.value;
                })
              }
            >
              <option value="default">{t("settings.deviceDefault")}</option>
              {devices.map((device) => (
                <option key={device.name} value={device.name}>
                  {device.name}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="audio-gain">
              {t("settings.gain")}: {draft.audio.gain.toFixed(1)}
            </label>
            <input
              id="audio-gain"
              type="range"
              min={0.1}
              max={4}
              step={0.1}
              value={draft.audio.gain}
              onChange={(event) =>
                patch((next) => {
                  next.audio.gain = Number(event.target.value);
                })
              }
            />
          </div>
          <div className="field">
            <label htmlFor="audio-duration">{t("settings.maxDuration")}</label>
            <input
              id="audio-duration"
              type="number"
              min={1}
              value={draft.audio.max_duration_secs}
              onChange={(event) =>
                patch((next) => {
                  next.audio.max_duration_secs = Number(event.target.value);
                })
              }
            />
          </div>
          <div className="field">
            <label htmlFor="audio-volume">
              {t("settings.soundVolume")}: {draft.audio.sound_volume.toFixed(2)}
            </label>
            <input
              id="audio-volume"
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={draft.audio.sound_volume}
              onChange={(event) =>
                patch((next) => {
                  next.audio.sound_volume = Number(event.target.value);
                })
              }
            />
          </div>
        </div>
        <Check
          label={t("settings.sounds")}
          checked={draft.audio.sounds}
          onChange={(value) =>
            patch((next) => {
              next.audio.sounds = value;
            })
          }
        />
        <Check
          label={t("settings.warnZeroLevel")}
          checked={draft.audio.warn_zero_level}
          onChange={(value) =>
            patch((next) => {
              next.audio.warn_zero_level = value;
            })
          }
        />
        <div className="row">
          <button
            type="button"
            onClick={() =>
              api
                .listDevices()
                .then(setDevices)
                .catch((err) => onError(asCommandError(err)))
            }
          >
            {t("common.refresh")}
          </button>
        </div>
      </Section>

      <Section title={t("settings.section.recognition")}>
        <div className="grid2">
          <div className="field">
            <label htmlFor="stt-engine">{t("settings.engine")}</label>
            <select
              id="stt-engine"
              value={draft.stt.engine}
              onChange={(event) =>
                patch((next) => {
                  next.stt.engine = event.target.value;
                })
              }
            >
              <option value="whisper-cpp">whisper-cpp</option>
              <option value="remote-openai">remote-openai</option>
            </select>
          </div>
          <div className="field">
            <label htmlFor="stt-model">{t("settings.model")}</label>
            <select
              id="stt-model"
              value={draft.stt.model}
              onChange={(event) =>
                patch((next) => {
                  next.stt.model = event.target.value;
                })
              }
            >
              {MODELS.map((model) => (
                <option key={model} value={model}>
                  {model}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="stt-language">{t("settings.sttLanguage")}</label>
            <input
              id="stt-language"
              type="text"
              value={draft.stt.language}
              onChange={(event) =>
                patch((next) => {
                  next.stt.language = event.target.value;
                })
              }
            />
          </div>
          <div className="field">
            <label htmlFor="stt-threads">{t("settings.threads")}</label>
            <input
              id="stt-threads"
              type="number"
              min={0}
              value={draft.stt.threads}
              onChange={(event) =>
                patch((next) => {
                  next.stt.threads = Number(event.target.value);
                })
              }
            />
          </div>
          <div className="field wide">
            <label htmlFor="stt-model-path">{t("settings.modelPath")}</label>
            <input
              id="stt-model-path"
              type="text"
              value={draft.stt.model_path}
              onChange={(event) =>
                patch((next) => {
                  next.stt.model_path = event.target.value;
                })
              }
            />
          </div>
        </div>

        <h3>{t("settings.allowedLanguages")}</h3>
        <div className="row">
          {draft.stt.allowed_languages.map((code) => (
            <button
              key={code}
              type="button"
              className="chip"
              aria-label={`${code} — ${t("common.delete")}`}
              onClick={() =>
                patch((next) => {
                  next.stt.allowed_languages = next.stt.allowed_languages.filter(
                    (item) => item !== code,
                  );
                })
              }
            >
              {code} ✕
            </button>
          ))}
        </div>
        <div className="row" style={{ marginTop: "0.5rem", alignItems: "flex-end" }}>
          <div className="field" style={{ marginBottom: 0, maxWidth: "8rem" }}>
            <label htmlFor="stt-new-language">{t("settings.addLanguage")}</label>
            <input
              id="stt-new-language"
              type="text"
              value={newLanguage}
              maxLength={5}
              onChange={(event) => setNewLanguage(event.target.value)}
            />
          </div>
          <button
            type="button"
            disabled={newLanguage.trim() === ""}
            onClick={() => {
              const code = newLanguage.trim().toLowerCase();
              patch((next) => {
                if (!next.stt.allowed_languages.includes(code)) {
                  next.stt.allowed_languages.push(code);
                }
              });
              setNewLanguage("");
            }}
          >
            {t("settings.addLanguage")}
          </button>
        </div>
        <Check
          label={t("settings.streamingPreview")}
          checked={draft.stt.streaming_preview}
          onChange={(value) =>
            patch((next) => {
              next.stt.streaming_preview = value;
            })
          }
        />
      </Section>

      <Section title={t("settings.section.post")}>
        <h3>{t("settings.rules")}</h3>
        {(
          [
            ["enabled", "settings.rules.enabled"],
            ["spoken_punctuation", "settings.rules.spokenPunctuation"],
            ["auto_punctuation", "settings.rules.autoPunctuation"],
            ["remove_fillers", "settings.rules.removeFillers"],
            ["remove_repeats", "settings.rules.removeRepeats"],
            ["numbers_as_digits", "settings.rules.numbersAsDigits"],
          ] as const
        ).map(([key, label]) => (
          <Check
            key={key}
            label={t(label)}
            checked={draft.rules[key]}
            onChange={(value) =>
              patch((next) => {
                next.rules[key] = value;
              })
            }
          />
        ))}

        <h3>{t("settings.llm")}</h3>
        <Check
          label={t("settings.llm.enabled")}
          checked={draft.llm.enabled}
          onChange={(value) =>
            patch((next) => {
              next.llm.enabled = value;
            })
          }
        />
        <div className="grid2">
          <div className="field">
            <label htmlFor="llm-provider">{t("settings.llm.provider")}</label>
            <select
              id="llm-provider"
              value={draft.llm.provider}
              onChange={(event) => {
                const provider = event.target.value;
                const preset = PROVIDERS[provider];
                setCloudAccepted(false);
                patch((next) => {
                  next.llm.provider = provider;
                  if (preset && provider !== "custom") {
                    next.llm.base_url = preset.base_url;
                    next.llm.model = preset.model;
                    if (preset.api_key_env) {
                      next.llm.api_key_env = preset.api_key_env;
                    }
                  }
                });
              }}
            >
              {Object.keys(PROVIDERS).map((provider) => (
                <option key={provider} value={provider}>
                  {provider}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="llm-model">{t("settings.llm.model")}</label>
            <input
              id="llm-model"
              type="text"
              value={draft.llm.model}
              onChange={(event) =>
                patch((next) => {
                  next.llm.model = event.target.value;
                })
              }
            />
          </div>
          <div className="field wide">
            <label htmlFor="llm-base-url">{t("settings.llm.baseUrl")}</label>
            <input
              id="llm-base-url"
              type="text"
              value={draft.llm.base_url}
              onChange={(event) =>
                patch((next) => {
                  next.llm.base_url = event.target.value;
                })
              }
            />
          </div>
          <div className="field">
            <label htmlFor="llm-key-env">{t("settings.llm.keyEnv")}</label>
            <input
              id="llm-key-env"
              type="text"
              value={draft.llm.api_key_env}
              onChange={(event) =>
                patch((next) => {
                  next.llm.api_key_env = event.target.value;
                })
              }
            />
          </div>
          <div className="field">
            <label htmlFor="llm-key-source">{t("settings.llm.keySource")}</label>
            <select
              id="llm-key-source"
              value={draft.llm.api_key_source}
              onChange={(event) =>
                patch((next) => {
                  next.llm.api_key_source = event.target.value;
                })
              }
            >
              <option value="keyring">keyring</option>
              <option value="env">env</option>
            </select>
          </div>
          <div className="field">
            <label htmlFor="llm-temperature">
              {t("settings.llm.temperature")}: {draft.llm.temperature.toFixed(2)}
            </label>
            <input
              id="llm-temperature"
              type="range"
              min={0}
              max={2}
              step={0.05}
              value={draft.llm.temperature}
              onChange={(event) =>
                patch((next) => {
                  next.llm.temperature = Number(event.target.value);
                })
              }
            />
          </div>
        </div>
        {cloudSelected && (
          <div className="notice warning" role="alert">
            <strong>{t("settings.llm.cloudWarning")}</strong>
            <Check
              label={t("settings.llm.cloudConfirm")}
              checked={cloudAccepted}
              onChange={setCloudAccepted}
            />
          </div>
        )}

        <div className="field">
          <label htmlFor="default-style">{t("settings.defaultStyle")}</label>
          <select
            id="default-style"
            value={draft.style.default}
            onChange={(event) =>
              patch((next) => {
                next.style.default = event.target.value;
              })
            }
          >
            {styles.map((style) => (
              <option key={style.id} value={style.id}>
                {style.name}
              </option>
            ))}
          </select>
        </div>
      </Section>

      <Section title={t("settings.section.output")}>
        <div className="grid2">
          <div className="field">
            <label htmlFor="output-mode">{t("settings.output.mode")}</label>
            <select
              id="output-mode"
              value={draft.output.mode}
              onChange={(event) =>
                patch((next) => {
                  next.output.mode = event.target.value;
                })
              }
            >
              {OUTPUT_MODES.map((mode) => (
                <option key={mode} value={mode}>
                  {t(`settings.output.${mode}`)}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="output-threshold">{t("settings.output.threshold")}</label>
            <input
              id="output-threshold"
              type="number"
              min={0}
              value={draft.output.auto_type_max_chars}
              onChange={(event) =>
                patch((next) => {
                  next.output.auto_type_max_chars = Number(event.target.value);
                })
              }
            />
          </div>
        </div>
        <Check
          label={t("settings.output.restore")}
          checked={draft.output.restore_clipboard}
          onChange={(value) =>
            patch((next) => {
              next.output.restore_clipboard = value;
            })
          }
        />
      </Section>

      <Section title={t("settings.section.privacy")}>
        <Check
          label={t("settings.privacy.includeText")}
          checked={draft.journal.include_text}
          onChange={(value) =>
            patch((next) => {
              next.journal.include_text = value;
            })
          }
        />
        <Check
          label={t("settings.privacy.noRecord")}
          checked={draft.privacy.no_record_mode}
          onChange={(value) =>
            patch((next) => {
              next.privacy.no_record_mode = value;
            })
          }
        />
        <Check
          label={t("settings.privacy.keepAudio")}
          checked={draft.journal.keep_audio}
          onChange={(value) =>
            patch((next) => {
              next.journal.keep_audio = value;
            })
          }
        />
        <Check
          label={t("settings.privacy.sendToLlm")}
          checked={draft.privacy.send_to_llm}
          onChange={(value) =>
            patch((next) => {
              next.privacy.send_to_llm = value;
            })
          }
        />
        <p className="small muted">{t("settings.privacy.telemetry")}</p>
      </Section>

      <section className="card">
        <div className="row">
          <button
            type="button"
            className="primary"
            onClick={() => void save()}
            disabled={blockedByCloud}
          >
            {t("common.save")}
          </button>
          <button type="button" className="danger" onClick={() => void reset()}>
            {t("common.reset")}
          </button>
          <button
            type="button"
            onClick={() =>
              api
                .exportConfig()
                .then(setExportedTo)
                .catch((err) => onError(asCommandError(err)))
            }
          >
            {t("common.export")}
          </button>
        </div>
        {exportedTo && (
          <p className="small muted">
            {t("stats.exported", { path: exportedTo })}
          </p>
        )}
        <div className="row" style={{ marginTop: "0.75rem", alignItems: "flex-end" }}>
          <div className="field" style={{ marginBottom: 0, flex: "1 1 16rem" }}>
            <label htmlFor="import-path">{t("settings.importPath")}</label>
            <input
              id="import-path"
              type="text"
              value={importPath}
              onChange={(event) => setImportPath(event.target.value)}
            />
          </div>
          <button
            type="button"
            disabled={importPath.trim() === ""}
            onClick={() =>
              api
                .importConfig(importPath.trim())
                .then((next) => {
                  setDraft(next);
                  setSaved(true);
                  return reloadConfig();
                })
                .catch((err) => {
                  const error = asCommandError(err);
                  setInvalid(error);
                  onError(error);
                })
            }
          >
            {t("common.import")}
          </button>
        </div>
      </section>
    </>
  );
}
