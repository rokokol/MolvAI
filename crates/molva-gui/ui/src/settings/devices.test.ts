// SPDX-License-Identifier: MIT
// The device list never becomes "no devices" because the daemon was slow to start

import { describe, expect, it } from "vitest";

import {
  afterDeviceFailure,
  afterDeviceSuccess,
  DEVICE_RETRY_LIMIT,
  deviceListState,
  initialDevices,
  shouldRetryDevices,
} from "./devices";

const microphone = { name: "Built-in Microphone", is_default: true, sample_rates: [48000] };

describe("deviceListState", () => {
  it("is loading before the first answer, not ready", () => {
    expect(deviceListState(initialDevices())).toBe("loading");
  });

  it("is ready when the daemon answers with an empty list", () => {
    expect(deviceListState(afterDeviceSuccess([]))).toBe("ready");
  });

  it("is an error after a failure that left no list", () => {
    const state = afterDeviceFailure(initialDevices(), "daemon is not running");
    expect(deviceListState(state)).toBe("error");
  });
});

describe("afterDeviceFailure", () => {
  it("keeps the reason and counts the attempt", () => {
    const state = afterDeviceFailure(initialDevices(), "socket missing");
    expect(state.error).toBe("socket missing");
    expect(state.attempt).toBe(1);
    expect(state.loaded).toBe(false);
  });

  it("keeps a list that already arrived", () => {
    const loaded = afterDeviceSuccess([microphone]);
    const state = afterDeviceFailure(loaded, "one slow answer");
    expect(state.devices).toEqual([microphone]);
    expect(deviceListState(state)).toBe("ready");
    expect(state.error).toBe("one slow answer");
  });
});

describe("shouldRetryDevices", () => {
  it("retries after a failure and stops after a success", () => {
    expect(shouldRetryDevices(afterDeviceFailure(initialDevices(), "not ready"))).toBe(true);
    expect(shouldRetryDevices(afterDeviceSuccess([microphone]))).toBe(false);
    expect(shouldRetryDevices(afterDeviceSuccess([]))).toBe(false);
  });

  it("stops after the attempt limit and leaves the Refresh button", () => {
    let state = initialDevices();
    for (let attempt = 0; attempt < DEVICE_RETRY_LIMIT; attempt += 1) {
      state = afterDeviceFailure(state, "still not ready");
    }
    expect(shouldRetryDevices(state)).toBe(false);
    expect(state.attempt).toBe(DEVICE_RETRY_LIMIT);
  });

  it("does not poll again over a list that is already shown", () => {
    const loaded = afterDeviceSuccess([microphone]);
    expect(shouldRetryDevices(afterDeviceFailure(loaded, "one slow answer"))).toBe(false);
  });
});
