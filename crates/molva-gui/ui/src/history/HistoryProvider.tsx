// SPDX-License-Identifier: MIT
// The loaded history lives above the tabs.
//
// Opening Settings and coming back must not empty the list or reset the filters: the query and
// the rows are window state, not view state. A new phrase marks the list stale instead of
// reloading it under the user's hands.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";

import { api, asCommandError } from "../api";
import type { CommandError, Entry } from "../types";

/** How long the query waits before the list is fetched again */
const QUERY_DEBOUNCE_MS = 200;

/** Midnight of the given date in ISO: the "from date" filter includes the whole day */
function dayStart(value: string): string | null {
  if (!value) {
    return null;
  }
  const date = new Date(`${value}T00:00:00`);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

/** The last millisecond of the given date */
function dayEnd(value: string): string | null {
  if (!value) {
    return null;
  }
  const date = new Date(`${value}T23:59:59.999`);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export interface HistoryState {
  entries: Entry[];
  apps: string[];
  query: string;
  setQuery: (value: string) => void;
  app: string;
  setApp: (value: string) => void;
  since: string;
  setSince: (value: string) => void;
  until: string;
  setUntil: (value: string) => void;
  loading: boolean;
  filtered: boolean;
  dataDir: string;
  copiedId: string | null;
  setCopiedId: (value: string | null) => void;
  /** Fetch again with the current filters, keeping the rows on screen meanwhile */
  reload: () => void;
  /** Fetch again only when a phrase arrived since the last load */
  reloadIfStale: () => void;
  remove: (id: string) => Promise<void>;
  clearAll: () => Promise<void>;
}

const HistoryContext = createContext<HistoryState | null>(null);

export function useHistoryState(): HistoryState {
  const value = useContext(HistoryContext);
  if (!value) {
    throw new Error("useHistoryState is used outside HistoryProvider");
  }
  return value;
}

export function HistoryProvider({
  onError,
  children,
}: {
  onError: (error: CommandError | null) => void;
  children: ReactNode;
}) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [apps, setApps] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [app, setApp] = useState("");
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const [loading, setLoading] = useState(true);
  const [dataDir, setDataDir] = useState("");
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const stale = useRef(false);
  const request = useRef(0);

  const load = useCallback(async () => {
    request.current += 1;
    const current = request.current;
    setLoading(true);
    try {
      const list = await api.historyList({
        query,
        app,
        since: dayStart(since),
        until: dayEnd(until),
      });
      // A slower answer to an older query must not overwrite a newer list
      if (current === request.current) {
        setEntries(list);
        stale.current = false;
        onError(null);
      }
    } catch (err) {
      if (current === request.current) {
        onError(asCommandError(err));
      }
    } finally {
      if (current === request.current) {
        setLoading(false);
      }
    }
  }, [query, app, since, until, onError]);

  // The query is typed, so it waits a moment; the other filters apply at once
  useEffect(() => {
    const timer = window.setTimeout(() => void load(), QUERY_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [load]);

  useEffect(() => {
    api.historyApps().then(setApps).catch(() => setApps([]));
    api.dataDirPath().then(setDataDir).catch(() => setDataDir(""));
  }, []);

  useEffect(() => {
    const unlisten = listen("molva://entry", () => {
      stale.current = true;
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const reload = useCallback(() => void load(), [load]);

  const reloadIfStale = useCallback(() => {
    if (stale.current) {
      void load();
    }
  }, [load]);

  const remove = useCallback(
    async (id: string) => {
      try {
        await api.historyDelete(id);
        setEntries((prev) => prev.filter((entry) => entry.id !== id));
      } catch (err) {
        onError(asCommandError(err));
      }
    },
    [onError],
  );

  const clearAll = useCallback(async () => {
    try {
      await api.historyClear();
      setEntries([]);
    } catch (err) {
      onError(asCommandError(err));
    }
  }, [onError]);

  const filtered = query.trim() !== "" || app !== "" || since !== "" || until !== "";

  const value = useMemo<HistoryState>(
    () => ({
      entries,
      apps,
      query,
      setQuery,
      app,
      setApp,
      since,
      setSince,
      until,
      setUntil,
      loading,
      filtered,
      dataDir,
      copiedId,
      setCopiedId,
      reload,
      reloadIfStale,
      remove,
      clearAll,
    }),
    [
      entries,
      apps,
      query,
      app,
      since,
      until,
      loading,
      filtered,
      dataDir,
      copiedId,
      reload,
      reloadIfStale,
      remove,
      clearAll,
    ],
  );

  return <HistoryContext.Provider value={value}>{children}</HistoryContext.Provider>;
}
