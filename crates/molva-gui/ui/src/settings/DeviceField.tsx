// SPDX-License-Identifier: MIT
// The input device: the daemon may still be starting, so a failed list is shown as a reason
// and retried, never turned into "no devices"

import { useCallback, useEffect, useRef, useState } from "react";

import { api, asCommandError } from "../api";
import { useI18n } from "../i18n";
import {
  afterDeviceFailure,
  afterDeviceSuccess,
  DEVICE_RETRY_MS,
  deviceListState,
  initialDevices,
  shouldRetryDevices,
} from "./devices";

interface Props {
  value: string;
  onChange: (device: string) => void;
  daemonRunning: boolean;
}

export default function DeviceField({ value, onChange, daemonRunning }: Props) {
  const { t } = useI18n();
  const [state, setState] = useState(initialDevices);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      setState(afterDeviceSuccess(await api.listDevices()));
    } catch (err) {
      setState((prev) => afterDeviceFailure(prev, asCommandError(err).message));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // A short retry while the list has never arrived: the daemon downloads the weights first,
  // and its socket opens only after that
  useEffect(() => {
    if (!shouldRetryDevices(state)) {
      return;
    }
    const timer = window.setTimeout(() => void load(), DEVICE_RETRY_MS);
    return () => window.clearTimeout(timer);
  }, [state, load]);

  // The daemon has just appeared: ask again at once instead of waiting for the timer
  const wasRunning = useRef(daemonRunning);
  useEffect(() => {
    if (daemonRunning && !wasRunning.current && state.error !== null) {
      void load();
    }
    wasRunning.current = daemonRunning;
  }, [daemonRunning, load, state.error]);

  const listState = deviceListState(state);
  const known = state.devices.some((device) => device.name === value);

  return (
    <div className="field">
      <label htmlFor="audio-device">{t("settings.device")}</label>
      <div className="field-inline">
        <select
          id="audio-device"
          value={value}
          onChange={(event) => onChange(event.target.value)}
          aria-describedby="audio-device-status"
        >
          <option value="default">{t("settings.deviceDefault")}</option>
          {state.devices.map((device) => (
            <option key={device.name} value={device.name}>
              {device.name}
              {device.is_default ? ` — ${t("settings.deviceSystemDefault")}` : ""}
            </option>
          ))}
          {value !== "default" && !known && <option value={value}>{value}</option>}
        </select>
        <button type="button" onClick={() => void load()} disabled={busy}>
          {t("common.refresh")}
        </button>
      </div>
      <p className="small muted" id="audio-device-status" aria-live="polite">
        {listState === "loading" && t("settings.deviceLoading")}
        {listState === "ready" &&
          (state.devices.length === 0
            ? t("settings.deviceNone")
            : t("settings.deviceCount", { count: state.devices.length }))}
        {listState === "error" && t("settings.deviceError")}
      </p>
      {listState === "error" && state.error && (
        <div className="notice error" role="alert">
          <p className="mono small">{state.error}</p>
          <p className="small">{t("settings.deviceErrorHint")}</p>
        </div>
      )}
    </div>
  );
}
