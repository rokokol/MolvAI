// SPDX-License-Identifier: MIT
// The input device list: a daemon that is not ready yet is a retry, never an empty list
//
// The old page asked once, swallowed the error and kept an empty array, so a slow daemon start
// looked like a machine with no microphone. The state below tells "not loaded yet" from
// "loaded, and the list is empty", and the page shows the reason meanwhile

import type { DeviceInfo } from "../types";

/** Attempts before the page stops and waits for the Refresh button */
export const DEVICE_RETRY_LIMIT = 20;
/** Pause between attempts while the list has never loaded */
export const DEVICE_RETRY_MS = 1500;

export interface DeviceState {
  devices: DeviceInfo[];
  /** The message of the last failure, `null` after a success */
  error: string | null;
  /** Attempts made, so the loop can stop */
  attempt: number;
  /** A list has arrived at least once */
  loaded: boolean;
}

export function initialDevices(): DeviceState {
  return { devices: [], error: null, attempt: 0, loaded: false };
}

/** The daemon answered: the list is the truth, even when it is empty */
export function afterDeviceSuccess(devices: DeviceInfo[]): DeviceState {
  return { devices, error: null, attempt: 0, loaded: true };
}

/**
 * The daemon did not answer: keep what the page already had, remember the reason and count
 * the attempt. A repeated failure does not erase a list that once arrived
 */
export function afterDeviceFailure(state: DeviceState, message: string): DeviceState {
  return {
    devices: state.devices,
    error: message,
    attempt: state.attempt + 1,
    loaded: state.loaded,
  };
}

/** Whether another attempt is worth making on its own */
export function shouldRetryDevices(state: DeviceState): boolean {
  if (state.error === null) {
    return false;
  }
  if (!state.loaded && state.attempt >= DEVICE_RETRY_LIMIT) {
    return false;
  }
  // A list that already arrived survives a failure: one slow answer is no reason to poll
  return !state.loaded;
}

/** What the select shows: a loading state differs from a machine with no devices */
export function deviceListState(state: DeviceState): "loading" | "ready" | "error" {
  if (state.error !== null && !state.loaded) {
    return "error";
  }
  return state.loaded ? "ready" : "loading";
}
