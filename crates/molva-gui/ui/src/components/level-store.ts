// SPDX-License-Identifier: MIT
// The microphone level arrives ten times a second. It lives outside React so only the meter
// repaints, instead of the whole window on every event

import { useSyncExternalStore } from "react";

let level = 0;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function snapshot(): number {
  return level;
}

export function publishLevel(value: number): void {
  if (level === value) {
    return;
  }
  level = value;
  for (const listener of listeners) {
    listener();
  }
}

export function useLevel(): number {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}
