// SPDX-License-Identifier: MIT
// The rows of the history: what one phrase shows, and the boundary that keeps a keystroke in the
// search box from redrawing a journal of thousands of phrases.

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { I18nContext, translate } from "../i18n";
import type { Entry } from "../types";
import HistoryList, { HistoryRow } from "./HistoryList";

const i18n = {
  lang: "en" as const,
  t: (key: string, vars?: Record<string, string | number>) => translate("en", key, vars),
};

function entry(id: string, text: string | null, extra: Partial<Entry> = {}): Entry {
  return {
    schema: 1,
    id,
    ts: "2026-09-01T10:00:00Z",
    session_id: "00000000-0000-0000-0000-000000000000",
    mode: "dictation",
    source: "mic",
    snippet: false,
    app: "kitty",
    language: "ru",
    audio_secs: 4,
    words: 10,
    wpm: 150,
    style: "cleanup",
    stt_engine: "whisper-cpp",
    stt_model: "small",
    llm_provider: null,
    llm_model: null,
    llm_used: false,
    local_llm: true,
    dict_hits: 0,
    inject_method: null,
    latency_ms: { stt: 10, rules: 1, llm: 0, inject: 1, total: 12 },
    tokens: null,
    llm_cost_usd: null,
    llm_calls: 0,
    llm_unpriced_calls: 0,
    error: null,
    text_raw: text,
    text_final: text,
    audio_path: null,
    segments: [],
    ...extra,
  } as Entry;
}

function render(entries: Entry[], copiedId: string | null = null) {
  return renderToStaticMarkup(
    <I18nContext.Provider value={i18n}>
      <HistoryList
        entries={entries}
        copiedId={copiedId}
        onCopied={() => undefined}
        onDelete={() => undefined}
      />
    </I18nContext.Provider>,
  );
}

describe("the list of phrases", () => {
  it("shows the STT provider and model alongside the file source", () => {
    const html = render([entry("a", "привет", {
      source: "file",
      stt_engine: "remote-openai",
      stt_provider: "groq",
      stt_model: "whisper-large-v3-turbo",
    })]);
    expect(html).toContain(translate("en", "history.files"));
    expect(html).toContain("STT · groq / whisper-large-v3-turbo");
    expect(html).not.toContain("remote-openai");
  });

  it("shows the numbers, the text and the two actions of a phrase", () => {
    const html = render([entry("a", "привет мир")]);

    expect(html.match(/<li class="entry split"/g)).toHaveLength(1);
    expect(html).toContain("привет мир");
    expect(html).toContain("150");
    expect(html).toContain(translate("en", "common.copy"));
    expect(html).toContain(translate("en", "common.delete"));
    expect(html).toContain("kitty");
  });

  it("keeps a phrase without text in the list with a copy button that does nothing", () => {
    const html = render([entry("a", null)]);

    expect(html).toContain(translate("en", "history.noText"));
    expect(html).toMatch(/disabled/);
  });

  it("marks the phrase that was just copied", () => {
    const html = render([entry("a", "раз"), entry("b", "два")], "b");

    expect(html.match(/Copied/g)).toHaveLength(1);
  });

  it("keeps the DOM bounded however long the journal is", () => {
    const entries = Array.from({ length: 10_000 }, (_, index) =>
      entry(String(index), `фраза ${index}`),
    );

    // A window, not the journal: the rows off the screen are padding, so the count does not
    // grow with the file and a keystroke in the search box redraws a screenful
    const painted = render(entries).match(/<li class="entry split"/g) ?? [];

    expect(painted.length).toBeGreaterThan(0);
    expect(painted.length).toBeLessThan(40);
    expect(render(entries)).toContain('padding-bottom');
  });

  it("paints a journal that fits one screen in one go", () => {
    const entries = Array.from({ length: 4 }, (_, index) =>
      entry(String(index), `фраза ${index}`),
    );

    const html = render(entries);
    expect(html.match(/<li class="entry split"/g)).toHaveLength(4);
    expect(html).toContain('data-index="3"');
  });
});

describe("the boundary of a row", () => {
  it("lets React skip a row whose phrase and copy state did not change", () => {
    // Without this boundary a keystroke in the search box redraws every row of the journal,
    // which is the cost this component exists to avoid
    expect(HistoryRow).toHaveProperty("$$typeof", Symbol.for("react.memo"));
  });
});
