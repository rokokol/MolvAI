// SPDX-License-Identifier: MIT
// What the two test buttons show after a test: the answer, the checks and the failures
//
// The buttons themselves live in the settings page and need a running command bridge; the result
// of a test is a pure view of a report, and that view is what these tests check

import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { I18nContext, translate } from "../i18n";
import type { LlmProbeReport, SttProbeReport } from "../types";
import { idleProbe, LlmProbeResult, SttProbeResult } from "./probe";

const i18n = {
  lang: "en" as const,
  t: (key: string, vars?: Record<string, string | number>) => translate("en", key, vars),
};

function render(node: ReactNode): string {
  return renderToStaticMarkup(<I18nContext.Provider value={i18n}>{node}</I18nContext.Provider>);
}

const localReport: LlmProbeReport = {
  provider: "ollama",
  model: "qwen3.5:4b",
  is_local: true,
  latency_ms: 120,
  attempts: 1,
  answer: "ok",
};

const sttReport: SttProbeReport = {
  engine: "transcribe-cpp",
  model: "gigaam-v3-e2e-rnnt-q4_k_m",
  ok: false,
  checks: [
    { id: "engine", ok: true, detail: "transcribe-cpp" },
    { id: "model", ok: false, detail: "no such file: /home/u/models/gigaam.gguf" },
    { id: "cli", ok: true, detail: "/usr/bin/transcribe-cli" },
  ],
};

describe("the model test result", () => {
  it("shows nothing before a test runs", () => {
    expect(render(<LlmProbeResult state={idleProbe()} />)).toBe("");
  });

  it("names the provider, the model, the time and the answer", () => {
    const html = render(<LlmProbeResult state={{ pending: false, report: localReport, error: null }} />);
    expect(html).toContain("ollama");
    expect(html).toContain("qwen3.5:4b");
    expect(html).toContain("120");
    expect(html).toContain(translate("en", "settings.llm.testAnswer", { answer: "ok" }));
    expect(html).toContain(translate("en", "settings.llm.testRetries", { attempts: 1 }));
    // A local model sends nothing away, so the cloud note must stay out
    expect(html).not.toContain(translate("en", "settings.llm.testCloudNote"));
  });

  it("says out loud that a cloud provider answered", () => {
    const html = render(
      <LlmProbeResult
        state={{ pending: false, report: { ...localReport, provider: "openrouter", is_local: false }, error: null }}
      />,
    );
    expect(html).toContain(translate("en", "settings.llm.testCloudNote"));
  });

  it("shows the failure with the next step", () => {
    const html = render(
      <LlmProbeResult
        state={{
          pending: false,
          report: null,
          error: { kind: "llm", message: "provider authorization error", hint: "Check the key" },
        }}
      />,
    );
    expect(html).toContain(translate("en", "settings.llm.testFailed"));
    expect(html).toContain("provider authorization error");
    expect(html).toContain("Check the key");
  });
});

describe("the recognition test result", () => {
  it("shows nothing before a test runs", () => {
    expect(render(<SttProbeResult state={idleProbe()} />)).toBe("");
  });

  it("lists every check with its title and its reason", () => {
    const html = render(<SttProbeResult state={{ pending: false, report: sttReport, error: null }} />);
    expect(html).toContain(translate("en", "settings.sttTest.failed"));
    for (const id of ["engine", "model", "cli"]) {
      expect(html).toContain(translate("en", `settings.sttTest.check.${id}`));
    }
    expect(html).toContain("/home/u/models/gigaam.gguf");
    expect(html).toContain("/usr/bin/transcribe-cli");
  });

  it("says when the draft can recognize", () => {
    const html = render(
      <SttProbeResult
        state={{
          pending: false,
          report: { ...sttReport, ok: true, checks: [{ id: "engine", ok: true, detail: "" }] },
          error: null,
        }}
      />,
    );
    expect(html).toContain(translate("en", "settings.sttTest.ok"));
    expect(html).not.toContain(translate("en", "settings.sttTest.failed"));
  });

  it("names the remote engine honestly", () => {
    const html = render(
      <SttProbeResult
        state={{
          pending: false,
          report: {
            engine: "remote-openai",
            model: "whisper-1",
            ok: false,
            checks: [
              { id: "remote", ok: false, detail: translate("en", "settings.sttTest.remoteUnavailable") },
            ],
          },
          error: null,
        }}
      />,
    );
    expect(html).toContain(translate("en", "settings.sttTest.check.remote"));
    expect(html).toContain(translate("en", "settings.sttTest.remoteUnavailable"));
  });
});
