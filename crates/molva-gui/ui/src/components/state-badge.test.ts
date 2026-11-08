// SPDX-License-Identifier: MIT
// The badge must not call the daemon ready while a file holds the recognizer

import { describe, expect, it } from "vitest";

import { badgeClass, stateLabel } from "./StateBadge";

/** The dictionaries answer with the key, so the assertions read like the table */
const t = (key: string) => key;

describe("badgeClass", () => {
  it("marks a recording and a processing phrase as busy", () => {
    expect(badgeClass("recording", true)).toBe("badge recording");
    expect(badgeClass("transcribing", true)).toBe("badge busy");
    expect(badgeClass("idle", true)).toBe("badge ready");
    expect(badgeClass("idle", false)).toBe("badge");
  });

  it("marks a file job as busy even when the state machine is idle", () => {
    expect(badgeClass("idle", true, "file")).toBe("badge busy");
    // The recording is the more important fact while both are true
    expect(badgeClass("recording", true, "file")).toBe("badge recording");
    expect(badgeClass(null, true, "file")).toBe("badge busy");
  });
});

describe("stateLabel", () => {
  it("names the work of the worker beside the state machine", () => {
    expect(stateLabel("idle", true, null, t)).toBe("state.idle");
    expect(stateLabel("idle", true, "file", t)).toBe("state.file");
    expect(stateLabel("recording", true, "file", t)).toBe("state.recording");
    expect(stateLabel("post_processing", true, null, t)).toBe("state.post_processing");
  });

  it("says nothing about a job without a daemon", () => {
    expect(stateLabel("idle", false, "file", t)).toBe("state.offline");
  });
});
