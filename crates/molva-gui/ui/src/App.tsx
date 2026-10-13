// SPDX-License-Identifier: MIT
// Window shell: left navigation, shared daemon state and a subscription to its events

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";

import { api, asCommandError } from "./api";
import { I18nContext, isLang, translate, type Lang } from "./i18n";
import type {
  CommandError,
  Config,
  DaemonPresence,
  DaemonState,
  Entry,
  Status,
} from "./types";
import Dashboard from "./views/Dashboard";
import Files from "./views/Files";
import History from "./views/History";
import Settings from "./views/Settings";
import Stats from "./views/Stats";

export type Tab = "dashboard" | "history" | "stats" | "files" | "settings";
export type Theme = "system" | "light" | "dark";

const TABS: Tab[] = ["dashboard", "history", "stats", "files", "settings"];
const THEME_KEY = "molva.theme";

/** Silence longer than this during recording is a reason to warn about a dead microphone */
const SILENCE_WARN_MS = 2000;
/** Below this level we treat the signal as absent */
const SILENCE_RMS = 0.005;

function storedTheme(): Theme {
  try {
    const value = localStorage.getItem(THEME_KEY);
    if (value === "light" || value === "dark" || value === "system") {
      return value;
    }
  } catch {
    // A private browser mode can forbid storage, and that is no reason to crash
  }
  return "system";
}

export default function App() {
  const [tab, setTab] = useState<Tab>("dashboard");
  const [config, setConfig] = useState<Config | null>(null);
  const [theme, setThemeState] = useState<Theme>(storedTheme);
  const [status, setStatus] = useState<Status | null>(null);
  const [level, setLevel] = useState(0);
  const [zeroLevel, setZeroLevel] = useState(false);
  const [lastEntry, setLastEntry] = useState<Entry | null>(null);
  const [hypothesis, setHypothesis] = useState("");
  const [error, setError] = useState<CommandError | null>(null);

  const [lang, setLang] = useState<Lang>("en");
  // Every settings change can change the language: ask again rather than resolve it here
  useEffect(() => {
    api
      .uiLanguage()
      .then((code) => {
        if (isLang(code)) {
          setLang(code);
        }
      })
      .catch(() => {
        // Without an answer the window stays in English, the fallback language
      });
  }, [config]);
  const t = useCallback(
    (key: string, vars?: Record<string, string | number>) => translate(lang, key, vars),
    [lang],
  );

  const lastSoundAt = useRef(Date.now());
  const daemonState: DaemonState | null = status?.state ?? null;
  const recording = daemonState === "recording";

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await api.getStatus());
    } catch (err) {
      setError(asCommandError(err));
    }
  }, []);

  const reloadConfig = useCallback(async () => {
    try {
      setConfig(await api.getConfig());
    } catch (err) {
      setError(asCommandError(err));
    }
  }, []);

  const setTheme = useCallback((next: Theme) => {
    setThemeState(next);
    try {
      localStorage.setItem(THEME_KEY, next);
    } catch {
      // The theme does not persist between launches, but the interface still works
    }
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    if (theme === "system") {
      root.removeAttribute("data-theme");
    } else {
      root.setAttribute("data-theme", theme);
    }
  }, [theme]);

  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  // The first launch opens the settings: without a config the user has nothing to look at
  useEffect(() => {
    void (async () => {
      await reloadConfig();
      await refreshStatus();
      try {
        if (await api.isFirstRun()) {
          setTab("settings");
        }
      } catch {
        // A missing reply must not block the window from opening
      }
      await emit("molva://ready");
    })();
  }, [reloadConfig, refreshStatus]);

  useEffect(() => {
    const unlisten = [
      listen<{ state: DaemonState }>("molva://state", (event) => {
        setStatus((prev) =>
          prev
            ? { ...prev, state: event.payload.state, daemon_running: true }
            : {
                daemon_running: true,
                daemon_ours: false,
                state: event.payload.state,
                style: null,
                hotkeys_paused: false,
              },
        );
        if (event.payload.state === "recording") {
          lastSoundAt.current = Date.now();
          setZeroLevel(false);
          setHypothesis("");
        } else {
          // Outside recording there is no level: the bar must drop, not freeze
          // at the last value and pretend to be a live microphone
          setLevel(0);
        }
      }),
      listen<number>("molva://level", (event) => {
        setLevel(event.payload);
        if (event.payload > SILENCE_RMS) {
          lastSoundAt.current = Date.now();
          setZeroLevel(false);
        }
      }),
      listen<string>("molva://hypothesis", (event) => setHypothesis(event.payload)),
      listen<Entry>("molva://entry", (event) => {
        setLastEntry(event.payload);
        setHypothesis("");
      }),
      listen<{ message: string; hint?: string }>("molva://error", (event) =>
        setError({ kind: "daemon", ...event.payload }),
      ),
      listen<DaemonPresence>("molva://daemon", (event) => {
        void refreshStatus();
        if (!event.payload.connected) {
          setLevel(0);
        }
      }),
      listen("molva://config", () => void reloadConfig()),
      listen<string>("molva://navigate", (event) => {
        const target = event.payload as Tab;
        if (TABS.includes(target)) {
          setTab(target);
        }
      }),
    ];
    return () => {
      void Promise.all(unlisten).then((fns) => fns.forEach((fn) => fn()));
    };
  }, [refreshStatus, reloadConfig]);

  // We notice silence by a timer: level events may not arrive during silence
  useEffect(() => {
    if (!recording || !config?.audio.warn_zero_level) {
      setZeroLevel(false);
      return;
    }
    const timer = window.setInterval(() => {
      setZeroLevel(Date.now() - lastSoundAt.current > SILENCE_WARN_MS);
    }, 500);
    return () => window.clearInterval(timer);
  }, [recording, config?.audio.warn_zero_level]);

  // The daemon state can change without events: something outside may have started it
  useEffect(() => {
    const timer = window.setInterval(() => void refreshStatus(), 5000);
    return () => window.clearInterval(timer);
  }, [refreshStatus]);

  const i18n = useMemo(() => ({ lang, t }), [lang, t]);

  const shared = {
    config,
    status,
    reloadConfig,
    refreshStatus,
    onError: setError,
  };

  return (
    <I18nContext.Provider value={i18n}>
      <div className="layout">
        <a className="skip-link" href="#main">
          {t("nav.label")}
        </a>
        <nav className="sidebar" aria-label={t("nav.label")}>
          <div className="brand">
            <strong>{t("app.title")}</strong>
            <span>{t("app.subtitle")}</span>
          </div>
          <div className="nav">
            {TABS.map((item) => (
              <button
                key={item}
                type="button"
                onClick={() => setTab(item)}
                aria-current={tab === item ? "page" : undefined}
              >
                {t(`nav.${item}`)}
              </button>
            ))}
          </div>
        </nav>
        <main className="content" id="main" tabIndex={-1}>
          {error && (
            <div className="notice error" role="alert">
              <strong>{t("error.title")}</strong>
              <p>{error.message}</p>
              {error.hint && (
                <p>
                  {t("common.nextStep")}: {error.hint}
                </p>
              )}
              <button type="button" className="ghost chip" onClick={() => setError(null)}>
                {t("common.close")}
              </button>
            </div>
          )}
          {tab === "dashboard" && (
            <Dashboard
              {...shared}
              level={level}
              zeroLevel={zeroLevel}
              lastEntry={lastEntry}
              hypothesis={hypothesis}
            />
          )}
          {tab === "history" && <History {...shared} />}
          {tab === "stats" && <Stats {...shared} />}
          {tab === "files" && <Files {...shared} />}
          {tab === "settings" && (
            <Settings {...shared} theme={theme} onThemeChange={setTheme} />
          )}
        </main>
      </div>
    </I18nContext.Provider>
  );
}

/** Properties shared by all tabs */
export interface ViewProps {
  config: Config | null;
  status: Status | null;
  reloadConfig: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  onError: (error: CommandError | null) => void;
}
