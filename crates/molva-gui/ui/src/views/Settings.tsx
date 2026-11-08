// SPDX-License-Identifier: MIT
// Settings: a form over Config, validation on the Rust side, TOML export and import
//
// The page reads as a few choices: a provider, a model, a language, a key source. Everything
// that only a careful setup needs sits under "Advanced", and no table that the core owns is
// repeated here: the models come from `models.toml` and the provider URLs from the Rust side

import { useCallback, useEffect, useState, type ReactNode } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import { AUTO_LANG, LANGUAGES, useI18n } from "../i18n";
import type { Theme, ViewProps } from "../App";
import type {
  CommandError,
  Config,
  CustomStyle,
  LlmProbeReport,
  LlmProviderOption,
  HyprlandBinds,
  ProviderKeyStatus,
  SttModelOption,
  SttProbeReport,
  StyleOption,
} from "../types";
import PathInput from "../files/PathInput";
import CopyButton from "../components/CopyButton";
import DeviceField from "../settings/DeviceField";
import HotkeyField from "../settings/HotkeyField";
import { StyleEditor } from "../settings/StyleEditor";
import { RECOGNITION_LANGUAGES } from "../settings/language";
import {
  keyField,
  keySourceChoices,
  storesInKeyring,
  storeUnavailable,
  warnsAboutClearText,
} from "../settings/keysource";
import {
  engineOfModel,
  humanSize,
  localEngines,
  modelFitsEngine,
  modelsForEngine,
  pickModel,
  REMOTE_ENGINE,
} from "../settings/providers";
import { mcpCommandLine, mcpProfiles } from "../settings/mcp";
import {
  idleProbe,
  LlmProbeResult,
  SttProbeResult,
  type ProbeState,
} from "../settings/probe";
import { styleIssues } from "../settings/styles";

interface Props extends ViewProps {
  theme: Theme;
  onThemeChange: (theme: Theme) => void;
  /** The config comes from the Nix store: the page is visible only for inspection */
  managed?: boolean;
}

const OUTPUT_MODES = ["auto", "paste", "type", "clipboard"];
const THEMES: Theme[] = ["system", "light", "dark"];
const HOTKEY_KEYS = ["push_to_talk", "toggle", "command", "cancel", "style_next"] as const;
const RULE_KEYS = [
  "enabled",
  "spoken_punctuation",
  "auto_punctuation",
  "remove_fillers",
  "remove_repeats",
  "numbers_as_digits",
] as const;

/** A model name that an OpenAI-compatible transcription endpoint serves, per provider */
const REMOTE_STT_MODELS: Record<string, string> = {
  openai: "whisper-1",
  openrouter: "openai/whisper-1",
};

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="card settings-section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

function Check({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint?: string;
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
      <span>
        {label}
        {hint && <span className="small muted">{hint}</span>}
      </span>
    </label>
  );
}

function engineLabel(id: string, t: (key: string) => string): string {
  if (id === "whisper-cpp") {
    return t("settings.engineName.whisper");
  }
  if (id === "transcribe-cpp") {
    return t("settings.engineName.gigaam");
  }
  if (id === REMOTE_ENGINE) {
    return t("settings.provider.remote");
  }
  return id;
}

function languageLabel(code: string, t: (key: string) => string): string {
  if (code === "") {
    return "";
  }
  const label = t(`language.${code}`);
  return label.startsWith("language.") ? code.toUpperCase() : label;
}

export default function Settings({
  config,
  status,
  reloadConfig,
  onError,
  theme,
  onThemeChange,
  managed = false,
}: Props) {
  const { t, lang } = useI18n();
  const [draft, setDraft] = useState<Config | null>(config);
  const [saved, setSaved] = useState(false);
  const [invalid, setInvalid] = useState<CommandError | null>(null);
  const [styles, setStyles] = useState<StyleOption[]>([]);
  const [builtins, setBuiltins] = useState<StyleOption[]>([]);
  const [models, setModels] = useState<SttModelOption[]>([]);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const [remoteAvailable, setRemoteAvailable] = useState(true);
  const [providers, setProviders] = useState<LlmProviderOption[]>([]);
  const [keyStatus, setKeyStatus] = useState<ProviderKeyStatus | null>(null);
  const [keyDraft, setKeyDraft] = useState("");
  const [keyBusy, setKeyBusy] = useState(false);
  // The test buttons work from the draft: nothing is saved before a test runs
  const [llmProbe, setLlmProbe] = useState<ProbeState<LlmProbeReport>>(idleProbe());
  const [sttProbe, setSttProbe] = useState<ProbeState<SttProbeReport>>(idleProbe());
  const [autostart, setAutostart] = useState(false);
  const [configPath, setConfigPath] = useState("");
  const [hyprland, setHyprland] = useState<HyprlandBinds | null>(null);
  const [includeCopied, setIncludeCopied] = useState(false);
  const [importPath, setImportPath] = useState("");
  const [exportedTo, setExportedTo] = useState("");
  const [newLanguage, setNewLanguage] = useState("ru");
  const [mcpProfile, setMcpProfile] = useState("claude-code");
  const [commandCopied, setCommandCopied] = useState(false);
  const [configCopied, setConfigCopied] = useState(false);
  const suggestPaths = useCallback(
    (input: string) => api.pathSuggestions(input, 10),
    [],
  );

  useEffect(() => {
    setDraft(config);
  }, [config]);

  useEffect(() => {
    api.getAutostart().then(setAutostart).catch(() => setAutostart(false));
    api.getConfigPath().then(setConfigPath).catch(() => setConfigPath(""));
    api
      .sttModels()
      .then((list) => {
        setModels(list);
        setModelsError(null);
      })
      .catch((err) => setModelsError(asCommandError(err).message));
    api.sttRemoteAvailable().then(setRemoteAvailable).catch(() => setRemoteAvailable(true));
    api.llmProviders().then(setProviders).catch(() => setProviders([]));
  }, []);

  // Built-in style names and the style menu come translated from the Rust side: ask again
  // when the language changes
  useEffect(() => {
    api.availableStyles().then(setStyles).catch(() => setStyles([]));
    api.builtinStyles().then(setBuiltins).catch(() => setBuiltins([]));
  }, [lang]);

  useEffect(() => {
    api.hyprlandBinds().then(setHyprland).catch(() => setHyprland(null));
  }, [config]);

  // Whether the key entry exists is a property of the provider and of the store, not of the
  // rest of the form: a keystroke elsewhere must not ask the store again
  const provider = draft?.llm.provider ?? "";
  const keySource = draft?.llm.api_key_source ?? "keyring";
  useEffect(() => {
    if (provider === "" || !storesInKeyring(keySource)) {
      setKeyStatus(null);
      return;
    }
    api
      .providerKeyStatus(provider, keySource)
      .then(setKeyStatus)
      .catch((err) =>
        setKeyStatus({ present: false, entry: "", detail: asCommandError(err).message }),
      );
  }, [provider, keySource]);

  const patch = useCallback((change: (next: Config) => void) => {
    setSaved(false);
    setDraft((prev) => {
      if (!prev) {
        return prev;
      }
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

  const llmCloud =
    !["ollama", "lmstudio", "custom"].includes(draft.llm.provider);
  const styleProblems = styleIssues(draft.style.custom);

  const keyStoreRefused = keyStatus !== null && storeUnavailable(keyStatus);

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

  // Both tests send the draft as it stands in the form. A test never writes the settings and
  // never starts a recording: the recognition test builds the engine and checks the files
  const testLlm = async () => {
    const sending = draft;
    setLlmProbe({ pending: true, report: null, error: null });
    try {
      setLlmProbe({ pending: false, report: await api.testLlm(sending), error: null });
    } catch (err) {
      setLlmProbe({ pending: false, report: null, error: asCommandError(err) });
    }
  };

  const testStt = async () => {
    const sending = draft;
    setSttProbe({ pending: true, report: null, error: null });
    try {
      setSttProbe({ pending: false, report: await api.testStt(sending), error: null });
    } catch (err) {
      setSttProbe({ pending: false, report: null, error: asCommandError(err) });
    }
  };

  const engine = engineOfModel(models, draft.stt.model) ?? draft.stt.engine;
  const selectedLanguages =
    draft.stt.language === "auto"
      ? draft.stt.allowed_languages
      : [draft.stt.language].filter((code) => code !== "");
  const availableLanguages = (RECOGNITION_LANGUAGES as readonly string[]).filter(
    (code) => !selectedLanguages.includes(code),
  );
  // The add menu must never keep a language that is already a chip: otherwise its value is
  // stale, the button looks disabled for no visible reason, and the first free language is
  // what the person expects to see
  useEffect(() => {
    if (
      availableLanguages.length > 0 &&
      !availableLanguages.includes(newLanguage)
    ) {
      setNewLanguage(availableLanguages[0]);
    }
  }, [availableLanguages, newLanguage]);
  const remoteProvider = draft.stt.remote.base_url.includes("openrouter")
    ? "openrouter"
    : "openai";
  const profile = mcpProfiles().find((item) => item.id === mcpProfile) ?? mcpProfiles()[0];

  const storeKey = async () => {
    setKeyBusy(true);
    try {
      const next = await api.storeProviderKey(provider, keySource, keyDraft);
      setKeyStatus(next);
      // The value leaves the page at once: it lives in the store, not in the window
      setKeyDraft("");
    } catch (err) {
      onError(asCommandError(err));
    } finally {
      setKeyBusy(false);
    }
  };

  return (
    <>
      <h1>{t("settings.title")}</h1>

      <section className="card settings-section settings-actions">
        {managed && (
          <div className="notice" role="status">
            <strong>{t("app.managed")}</strong>
            <p>{t("app.managedHint")}</p>
          </div>
        )}
        {styleProblems.length > 0 && (
          <div className="notice warning" role="alert">
            {t("settings.styles.blocked")}
          </div>
        )}
        <div className="row">
          <button
            type="button"
            className="primary"
            onClick={() => void save()}
            disabled={managed || styleProblems.length > 0}
          >
            {t("common.save")}
          </button>
          <button
            type="button"
            className="danger"
            onClick={() => void reset()}
            disabled={managed}
          >
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
          <p className="small muted">{t("stats.exported", { path: exportedTo })}</p>
        )}
        <div className="field-row">
          <div className="field">
            <label htmlFor="import-path">{t("settings.importPath")}</label>
            <PathInput
              id="import-path"
              value={importPath}
              suggest={suggestPaths}
              onChange={setImportPath}
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
        <div className="formgrid">
          <div className="field">
            <label htmlFor="ui-language">{t("settings.language")}</label>
            <select
              id="ui-language"
              value={draft.ui_language}
              onChange={(event) =>
                patch((next) => {
                  next.ui_language = event.target.value;
                })
              }
            >
              <option value={AUTO_LANG}>{t("settings.languageAuto")}</option>
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
        <p className="small muted">{t("settings.hotkeys.hint")}</p>
        <div className="formgrid">
          {HOTKEY_KEYS.map((key) => (
            <HotkeyField
              key={key}
              id={`hotkey-${key}`}
              label={t(`settings.hotkeys.${key === "push_to_talk" ? "pushToTalk" : key.replace("style_next", "styleNext")}`)}
              value={draft.hotkeys[key]}
              onChange={(value) =>
                patch((next) => {
                  next.hotkeys[key] = value;
                })
              }
            />
          ))}
        </div>

        {hyprland?.session && (
          <details className="help">
            <summary>{t("settings.wayland.title")}</summary>
            <p>{t("settings.wayland.body")}</p>
            {hyprland.written && (
              <>
                <p className="small">{t("settings.wayland.written", { path: hyprland.path })}</p>
                <p>{t("settings.wayland.include")}</p>
                <div className="copy-row">
                  <code className="mono">{hyprland.include_line}</code>
                  <CopyButton
                    copied={includeCopied}
                    label={t("common.copy")}
                    copiedLabel={t("common.copied")}
                    onClick={async () =>
                      setIncludeCopied(await copyToClipboard(hyprland.include_line))
                    }
                  />
                </div>
              </>
            )}
          </details>
        )}
      </Section>

      <Section title={t("settings.section.audio")}>
        <DeviceField
          value={draft.audio.device}
          daemonRunning={status?.daemon_running ?? false}
          onChange={(device) =>
            patch((next) => {
              next.audio.device = device;
            })
          }
        />
        <div className="formgrid">
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
        <div className="check-list">
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
        </div>
      </Section>

      <Section title={t("settings.section.recognition")}>
        {modelsError && (
          <div className="notice warning" role="alert">
            <p>{t("settings.modelsError")}</p>
            <p className="mono small">{modelsError}</p>
          </div>
        )}

        <div className="formgrid">
          <div className="field">
            <label htmlFor="stt-engine">{t("settings.engine")}</label>
            <select
              id="stt-engine"
              value={engine}
              onChange={(event) => {
                const nextEngine = event.target.value;
                patch((next) => {
                  next.stt.engine = nextEngine;
                  if (nextEngine !== REMOTE_ENGINE) {
                    next.stt.model = pickModel(models, nextEngine, next.stt.model);
                  }
                });
              }}
            >
              {(localEngines(models).includes(engine)
                ? localEngines(models)
                : [...localEngines(models), engine]
              ).map((id) => (
                <option key={id} value={id}>
                  {engineLabel(id, t)}
                </option>
              ))}
              <option value={REMOTE_ENGINE} disabled={!remoteAvailable}>
                {t("settings.provider.remote")}
              </option>
            </select>
            {engine === REMOTE_ENGINE && !remoteAvailable && (
              <p className="small warning-text">{t("settings.provider.remoteOff")}</p>
            )}
          </div>

          {engine !== REMOTE_ENGINE && (
            <div className="field">
              <label htmlFor="stt-model">{t("settings.model")}</label>
              <select
                id="stt-model"
                value={draft.stt.model}
                onChange={(event) => {
                  const nextModel = event.target.value;
                  const nextEngine = engineOfModel(models, nextModel);
                  patch((next) => {
                    next.stt.model = nextModel;
                    if (nextEngine) {
                      next.stt.engine = nextEngine;
                    }
                  });
                }}
              >
                {modelsForEngine(models, engine).map((model) => (
                  <option key={model.name} value={model.name}>
                    {model.name} — {humanSize(model.size_bytes)}
                    {model.language ? ` — ${model.language.toUpperCase()}` : ""}
                  </option>
                ))}
                {models.length > 0 &&
                  !modelFitsEngine(models, draft.stt.model, engine) && (
                    <option value={draft.stt.model}>{draft.stt.model}</option>
                  )}
              </select>
              {models.some((model) => model.name === draft.stt.model) && (
                <p className="small muted">
                  {models.find((model) => model.name === draft.stt.model)?.installed
                    ? t("settings.modelOnDisk")
                    : t("settings.modelWillDownload")}
                </p>
              )}
              {!modelFitsEngine(models, draft.stt.model, engine) && (
                <p className="small warning-text">{t("settings.modelMismatch")}</p>
              )}
            </div>
          )}
        </div>

        <div className="field">
          <button type="button" disabled={sttProbe.pending} onClick={() => void testStt()}>
            {sttProbe.pending
              ? t("settings.sttTest.testing")
              : t("settings.sttTest.button")}
          </button>
          <p className="small muted">{t("settings.sttTest.hint")}</p>
          <SttProbeResult state={sttProbe} />
        </div>

        {engine === REMOTE_ENGINE ? (
          <>
            <div className="notice warning" role="alert">
              <strong>{t("settings.remote.off")}</strong>
              <p>{t("settings.remote.offHint")}</p>
            </div>
            <div className="formgrid">
              <div className="field">
                <label htmlFor="remote-provider">{t("settings.llm.provider")}</label>
                <select
                  id="remote-provider"
                  value={remoteProvider}
                  disabled={!remoteAvailable}
                  onChange={(event) => {
                    const id = event.target.value;
                    const preset = providers.find((item) => item.id === id);
                    patch((next) => {
                      next.stt.remote.base_url = preset?.base_url ?? next.stt.remote.base_url;
                      next.stt.remote.model = REMOTE_STT_MODELS[id] ?? next.stt.remote.model;
                      next.stt.remote.api_key_env =
                        preset?.api_key_env ?? next.stt.remote.api_key_env;
                    });
                  }}
                >
                  {Object.keys(REMOTE_STT_MODELS).map((id) => (
                    <option key={id} value={id}>
                      {id === "openai" ? "OpenAI" : "OpenRouter"}
                    </option>
                  ))}
                </select>
              </div>
              <div className="field">
                <label htmlFor="remote-model">{t("settings.model")}</label>
                <input
                  id="remote-model"
                  type="text"
                  disabled={!remoteAvailable}
                  value={draft.stt.remote.model}
                  onChange={(event) =>
                    patch((next) => {
                      next.stt.remote.model = event.target.value;
                    })
                  }
                />
              </div>
              <div className="field wide">
                <label htmlFor="remote-base-url">{t("settings.llm.baseUrl")}</label>
                <input
                  id="remote-base-url"
                  type="text"
                  disabled={!remoteAvailable}
                  value={draft.stt.remote.base_url}
                  onChange={(event) =>
                    patch((next) => {
                      next.stt.remote.base_url = event.target.value;
                    })
                  }
                />
              </div>
              <div className="field">
                <label htmlFor="remote-key-source">{t("settings.llm.keySource")}</label>
                <select
                  id="remote-key-source"
                  disabled={!remoteAvailable}
                  value={draft.stt.remote.api_key_source}
                  onChange={(event) =>
                    patch((next) => {
                      next.stt.remote.api_key_source = event.target.value;
                    })
                  }
                >
                  <option value="keyring">{t("settings.keySource.keyring")}</option>
                  <option value="env">{t("settings.keySource.env")}</option>
                  <option value="none">{t("settings.keySource.none")}</option>
                </select>
              </div>
              <div className="field">
                <label htmlFor="remote-key-env">{t("settings.llm.keyEnv")}</label>
                <input
                  id="remote-key-env"
                  type="text"
                  disabled={!remoteAvailable}
                  value={draft.stt.remote.api_key_env}
                  onChange={(event) =>
                    patch((next) => {
                      next.stt.remote.api_key_env = event.target.value;
                    })
                  }
                />
              </div>
            </div>
          </>
        ) : (
          <div className="field wide">
            <label htmlFor="stt-model-path">{t("settings.modelPath")}</label>
            <PathInput
              id="stt-model-path"
              value={draft.stt.model_path}
              suggest={suggestPaths}
              onChange={(value) =>
                patch((next) => {
                  next.stt.model_path = value;
                })
              }
            />
            <p className="small muted">{t("settings.modelPathHint")}</p>
          </div>
        )}

        <div className="field wide">
          <label>{t("settings.sttLanguage")}</label>
          <p className="small muted">{t("settings.sttLanguageHint")}</p>
          <div className="chip-row">
            {selectedLanguages.map((code) => (
              <button
                key={code}
                type="button"
                className="chip"
                aria-label={`${code} — ${t("common.delete")}`}
                onClick={() => {
                  const next = selectedLanguages.filter((item) => item !== code);
                  patch((draftConfig) => {
                    draftConfig.stt.allowed_languages = next;
                    draftConfig.stt.language =
                      next.length === 1 ? next[0] : "auto";
                  });
                }}
              >
                {languageLabel(code, t)} ✕
              </button>
            ))}
            <select
              aria-label={t("settings.addLanguage")}
              value={newLanguage}
              disabled={availableLanguages.length === 0}
              onChange={(event) => setNewLanguage(event.target.value)}
            >
              {availableLanguages.map((code) => (
                <option key={code} value={code}>
                  {languageLabel(code, t)}
                </option>
              ))}
            </select>
            <button
              type="button"
              disabled={!availableLanguages.includes(newLanguage)}
              onClick={() => {
                const next = [...selectedLanguages, newLanguage];
                patch((draftConfig) => {
                  draftConfig.stt.allowed_languages = next;
                  draftConfig.stt.language = next.length === 1 ? next[0] : "auto";
                });
              }}
            >
              {t("settings.addLanguage")}
            </button>
          </div>
          {selectedLanguages.length > 1 && (
            <p className="small muted">{t("settings.allowedLanguagesHint")}</p>
          )}
        </div>

        <details className="help">
          <summary>{t("settings.advanced")}</summary>
          <div className="formgrid">
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
              <label htmlFor="stt-transcribe-cli">{t("settings.transcribeCli")}</label>
              <PathInput
                id="stt-transcribe-cli"
                value={draft.stt.transcribe_cli}
                suggest={suggestPaths}
                onChange={(value) =>
                  patch((next) => {
                    next.stt.transcribe_cli = value;
                  })
                }
              />
              <p className="small muted">{t("settings.transcribeCliHint")}</p>
            </div>
            <div className="field wide">
              <label htmlFor="stt-models-mirror">{t("settings.modelsMirror")}</label>
              <input
                id="stt-models-mirror"
                type="url"
                placeholder="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
                value={draft.stt.models_mirror}
                onChange={(event) =>
                  patch((next) => {
                    next.stt.models_mirror = event.target.value;
                  })
                }
              />
              <p className="small muted">{t("settings.modelsMirrorHint")}</p>
            </div>
          </div>
          <div className="check-list">
            <Check
              label={t("settings.streamingPreview")}
              checked={draft.stt.streaming_preview}
              onChange={(value) =>
                patch((next) => {
                  next.stt.streaming_preview = value;
                })
              }
            />
            <Check
              label={t("settings.autoPull")}
              hint={t("settings.autoPullHint")}
              checked={draft.stt.auto_pull}
              onChange={(value) =>
                patch((next) => {
                  next.stt.auto_pull = value;
                })
              }
            />
            <Check
              label={t("settings.chunked")}
              checked={draft.stt.chunked}
              onChange={(value) =>
                patch((next) => {
                  next.stt.chunked = value;
                })
              }
            />
          </div>
        </details>
      </Section>

      <Section title={t("settings.section.post")}>
        <h3>{t("settings.rules")}</h3>
        <div className="check-list">
          {RULE_KEYS.map((key) => (
            <Check
              key={key}
              label={t(`settings.rules.${key === "enabled" ? "enabled" : key.replace(/_([a-z])/g, (_m, letter: string) => letter.toUpperCase())}`)}
              checked={draft.rules[key]}
              onChange={(value) =>
                patch((next) => {
                  next.rules[key] = value;
                })
              }
            />
          ))}
        </div>

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
        <div className="formgrid">
          <div className="field">
            <label htmlFor="llm-provider">{t("settings.llm.provider")}</label>
            <select
              id="llm-provider"
              value={draft.llm.provider}
              onChange={(event) => {
                const id = event.target.value;
                const preset = providers.find((item) => item.id === id);
                patch((next) => {
                  next.llm.provider = id;
                  if (preset) {
                    next.llm.base_url = preset.base_url;
                    if (preset.model !== "") {
                      next.llm.model = preset.model;
                    }
                    if (preset.api_key_env !== "") {
                      next.llm.api_key_env = preset.api_key_env;
                    }
                  }
                });
              }}
            >
              {providers.map((item) => (
                <option key={item.id} value={item.id}>
                  {t(`settings.llmProvider.${item.id}`, { id: item.id })}
                </option>
              ))}
            </select>
            {providers.find((item) => item.id === draft.llm.provider)?.is_local && (
              <p className="small muted">{t("settings.llm.localHint")}</p>
            )}
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
        </div>

        <div className="field">
          <label htmlFor="llm-reasoning">{t("settings.llm.reasoning")}</label>
          <select
            id="llm-reasoning"
            value={draft.llm.reasoning_effort}
            onChange={(event) =>
              patch((next) => {
                next.llm.reasoning_effort = event.target.value;
              })
            }
          >
            <option value="none">{t("settings.llm.reasoningNone")}</option>
            <option value="auto">{t("settings.llm.reasoningAuto")}</option>
            <option value="">{t("settings.llm.reasoningOff")}</option>
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
          </select>
          <p className="small muted">{t("settings.llm.reasoningHint")}</p>
        </div>

        {draft.llm.provider === "openrouter" && (
          <>
            <h4>{t("settings.llm.hosters")}</h4>
            <p className="small muted">{t("settings.llm.hostersHint")}</p>
            <div className="formgrid">
              <div className="field">
                <label htmlFor="llm-provider-order">
                  {t("settings.llm.hostersOrder")}
                </label>
                <input
                  id="llm-provider-order"
                  type="text"
                  placeholder="groq, together"
                  value={draft.llm.provider_order}
                  onChange={(event) =>
                    patch((next) => {
                      next.llm.provider_order = event.target.value;
                    })
                  }
                />
                <p className="small muted">{t("settings.llm.hostersOrderHint")}</p>
              </div>
              <div className="field">
                <Check
                  label={t("settings.llm.allowFallbacks")}
                  hint={t("settings.llm.allowFallbacksHint")}
                  checked={draft.llm.allow_fallbacks}
                  onChange={(value) =>
                    patch((next) => {
                      next.llm.allow_fallbacks = value;
                    })
                  }
                />
                <Check
                  label={t("settings.llm.requireParameters")}
                  hint={t("settings.llm.requireParametersHint")}
                  checked={draft.llm.require_parameters}
                  onChange={(value) =>
                    patch((next) => {
                      next.llm.require_parameters = value;
                    })
                  }
                />
              </div>
            </div>
          </>
        )}

        <div className="field">
          <button type="button" disabled={llmProbe.pending} onClick={() => void testLlm()}>
            {llmProbe.pending ? t("settings.llm.testing") : t("settings.llm.test")}
          </button>
          <p className="small muted">{t("settings.llm.testHint")}</p>
          <LlmProbeResult state={llmProbe} />
        </div>

        {llmCloud && (
          <>
            <h4>{t("settings.llm.privacy")}</h4>
            <div className="formgrid">
              <div className="field">
                <label htmlFor="llm-data-collection">
                  {t("settings.llm.dataCollection")}
                </label>
                <select
                  id="llm-data-collection"
                  value={draft.llm.data_collection}
                  onChange={(event) =>
                    patch((next) => {
                      next.llm.data_collection = event.target.value;
                    })
                  }
                >
                  <option value="deny">{t("settings.llm.dataCollectionDeny")}</option>
                  <option value="allow">{t("settings.llm.dataCollectionAllow")}</option>
                </select>
                <p className="small muted">{t("settings.llm.dataCollectionHint")}</p>
              </div>
              <div className="field">
                <Check
                  label={t("settings.llm.zdr")}
                  hint={t("settings.llm.zdrHint")}
                  checked={draft.llm.zdr}
                  onChange={(value) =>
                    patch((next) => {
                      next.llm.zdr = value;
                    })
                  }
                />
              </div>
            </div>
          </>
        )}

        <details className="help">
          <summary>{t("settings.llm.connection")}</summary>
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
            <label htmlFor="llm-key-source">{t("settings.llm.keySource")}</label>
            <select
              id="llm-key-source"
              value={draft.llm.api_key_source}
              onChange={(event) => {
                const source = event.target.value;
                patch((next) => {
                  next.llm.api_key_source = source;
                });
              }}
            >
              {keySourceChoices(draft.llm.api_key_source).map((source) => (
                <option key={source} value={source}>
                  {t(`settings.keySource.${keyField(source)}`, { source })}
                </option>
              ))}
            </select>
            <p className="small muted">{t(`settings.keySourceHint.${keyField(keySource)}`)}</p>
          </div>

          {keyField(keySource) === "store" && (
            <div className="field wide">
              <label htmlFor="llm-key-value">{t("settings.llm.keyValue")}</label>
              <div className="field-inline">
                <input
                  id="llm-key-value"
                  type="password"
                  autoComplete="off"
                  value={keyDraft}
                  onChange={(event) => setKeyDraft(event.target.value)}
                />
                <button
                  type="button"
                  className="primary"
                  disabled={keyBusy || keyDraft.trim() === ""}
                  onClick={() => void storeKey()}
                >
                  {t("settings.llm.keyStore")}
                </button>
                <button
                  type="button"
                  disabled={keyBusy || keyStatus?.present !== true}
                  onClick={() =>
                    api
                      .removeProviderKey(provider, keySource)
                      .then(setKeyStatus)
                      .catch((err) => onError(asCommandError(err)))
                  }
                >
                  {t("settings.llm.keyRemove")}
                </button>
              </div>
              <p className="small" aria-live="polite">
                {keyStatus?.present
                  ? t("settings.llm.keyPresent", { entry: keyStatus.entry })
                  : t("settings.llm.keyMissing")}
              </p>
              {keyStoreRefused && keyStatus?.detail && (
                <div className="notice warning" role="alert">
                  <p>{t("settings.llm.keyStoreRefused")}</p>
                  <p className="mono small">{keyStatus.detail}</p>
                </div>
              )}
            </div>
          )}

          {warnsAboutClearText(keySource) && (
            <>
              <div className="notice warning" role="alert">
                <p>{t("settings.keySource.plainWarning")}</p>
              </div>
              <div className="field wide">
                <label htmlFor="llm-key-plain">{t("settings.llm.keyValue")}</label>
                <input
                  id="llm-key-plain"
                  type="password"
                  autoComplete="off"
                  value={draft.llm.api_key_plain}
                  onChange={(event) =>
                    patch((next) => {
                      next.llm.api_key_plain = event.target.value;
                    })
                  }
                />
              </div>
            </>
          )}

          {keyField(keySource) === "env" && (
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
          )}

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

        </details>

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
              <option
                key={style.id}
                value={style.id}
                disabled={!draft.llm.enabled && style.id !== "verbatim"}
              >
                {style.name}
              </option>
            ))}
          </select>
        </div>

        <StyleEditor
          styles={draft.style.custom}
          builtins={builtins}
          onChange={(next: CustomStyle[]) =>
            patch((configNext) => {
              configNext.style.custom = next;
            })
          }
        />
      </Section>

      <Section title={t("settings.section.output")}>
        <div className="formgrid">
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

      <Section title={t("settings.section.mcp")}>
        <Check
          label={t("settings.mcp.enabled")}
          hint={t("settings.mcp.enabledHint")}
          checked={draft.mcp.enabled}
          onChange={(value) =>
            patch((next) => {
              next.mcp.enabled = value;
            })
          }
        />
        <div className="field wide">
          <label>{t("settings.mcp.command")}</label>
          <div className="copy-row">
            <code className="mono">{mcpCommandLine()}</code>
            <CopyButton
              copied={commandCopied}
              label={t("common.copy")}
              copiedLabel={t("common.copied")}
              onClick={async () => setCommandCopied(await copyToClipboard(mcpCommandLine()))}
            />
          </div>
          <p className="small muted">{t("settings.mcp.stdioHint")}</p>
        </div>
        <div className="formgrid">
          <div className="field">
            <label htmlFor="mcp-profile">{t("settings.mcp.harness")}</label>
            <select
              id="mcp-profile"
              value={profile.id}
              onChange={(event) => {
                setConfigCopied(false);
                setMcpProfile(event.target.value);
              }}
            >
              {mcpProfiles().map((item) => (
                <option key={item.id} value={item.id}>
                  {t(item.labelKey)}
                </option>
              ))}
            </select>
            {profile.command === undefined && profile.file !== "" && (
              <p className="small muted">{t("settings.mcp.file", { file: profile.file })}</p>
            )}
          </div>
        </div>
        {profile.command ? (
          <div className="copy-row">
            <code className="mono">{profile.command}</code>
            <CopyButton
              copied={commandCopied}
              label={t("common.copy")}
              copiedLabel={t("common.copied")}
              onClick={async () =>
                setCommandCopied(await copyToClipboard(profile.command ?? ""))
              }
            />
          </div>
        ) : (
          <>
            <pre className="codeblock">{profile.config}</pre>
            <CopyButton
              copied={configCopied}
              label={t("common.copy")}
              copiedLabel={t("common.copied")}
              onClick={async () => setConfigCopied(await copyToClipboard(profile.config))}
            />
          </>
        )}
      </Section>

      <Section title={t("settings.section.privacy")}>
        <div className="check-list">
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
          <Check
            label={t("settings.privacy.cloudConsent")}
            checked={draft.privacy.cloud_consent}
            onChange={(value) =>
              patch((next) => {
                next.privacy.cloud_consent = value;
              })
            }
          />
        </div>
        <p className="small muted">{t("settings.privacy.telemetry")}</p>
      </Section>

    </>
  );
}
