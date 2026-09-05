// SPDX-License-Identifier: MIT
// Каркас окна: левая навигация, общее состояние демона и подписка на его события.

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

/** Тишина дольше этого во время записи — повод предупредить о мёртвом микрофоне. */
const SILENCE_WARN_MS = 2000;
/** Ниже этого уровня сигнал считаем отсутствующим. */
const SILENCE_RMS = 0.005;

function storedTheme(): Theme {
  try {
    const value = localStorage.getItem(THEME_KEY);
    if (value === "light" || value === "dark" || value === "system") {
      return value;
    }
  } catch {
    // Приватный режим браузера может запрещать хранилище — это не повод падать.
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

  const lang: Lang = config && isLang(config.ui_language) ? config.ui_language : "ru";
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
      // Тема не сохранится между запусками — интерфейс всё равно работает.
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

  // Первый запуск открывает настройки: без конфига пользователю нечего смотреть.
  useEffect(() => {
    void (async () => {
      await reloadConfig();
      await refreshStatus();
      try {
        if (await api.isFirstRun()) {
          setTab("settings");
        }
      } catch {
        // Отсутствие ответа не должно мешать открыть окно.
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
          // Вне записи уровня нет: полоска обязана опуститься, а не замереть
          // на последнем значении, изображая живой микрофон.
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

  // Тишину замечаем по таймеру: события уровня во время молчания могут и не приходить.
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

  // Состояние демона может измениться и без событий: он мог быть запущен снаружи.
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

/** Общие свойства всех вкладок. */
export interface ViewProps {
  config: Config | null;
  status: Status | null;
  reloadConfig: () => Promise<void>;
  refreshStatus: () => Promise<void>;
  onError: (error: CommandError | null) => void;
}
