// SPDX-License-Identifier: MIT
// Markup tests of the Files tab. React renders the components without a browser here, which is
// enough for what the reader of a screen sees first: which rows exist, what the bar claims and
// which tabs the navigation offers.

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import App from "../App";
import { I18nContext, translate } from "../i18n";
import { renderFileResult, renderSegment } from "../format";
import type { FileResult } from "../types";
import Files from "../views/Files";
import { FilesProvider } from "./FilesProvider";
import JobCell from "./JobCell";
import { newJob } from "./jobs";
import PathInput from "./PathInput";
import ProgressBar from "./ProgressBar";
import Transcript from "./Transcript";

const i18n = { lang: "en" as const, t: (key: string, vars?: Record<string, string | number>) => translate("en", key, vars) };

function wrap(node: React.ReactNode) {
  return <I18nContext.Provider value={i18n}>{node}</I18nContext.Provider>;
}

const result: FileResult = {
  file: "/media/lecture.mp4",
  text: "привет мир",
  segments: [
    { start_ms: 0, end_ms: 2500, text: " привет" },
    { start_ms: 2500, end_ms: 4000, text: "мир" },
  ],
  audio_secs: 4,
  latency_ms: 10,
  language: "ru",
};

describe("Transcript", () => {
  it("shows every segment with its start, end and text", () => {
    const html = renderToStaticMarkup(
      wrap(
        <Transcript
          result={result}
          timecodes
          currentMs={null}
          onSeek={() => undefined}
          onOpen={() => undefined}
        />,
      ),
    );
    expect(html).toContain("00:00.000");
    expect(html).toContain("00:02.500");
    expect(html).toContain("00:04.000");
    expect(html).toContain("привет");
    expect(html).toContain("мир");
    expect(html).toContain(translate("en", "files.copyAll"));
    expect(html).toContain(translate("en", "files.searchInTranscript"));
  });

  it("marks the row the playback position is inside", () => {
    const html = renderToStaticMarkup(
      wrap(
        <Transcript
          result={result}
          timecodes
          currentMs={3000}
          onSeek={() => undefined}
          onOpen={() => undefined}
        />,
      ),
    );
    expect(html.match(/class="segment active"/g)).toHaveLength(1);
    expect(html).toContain('aria-current="true"');
  });

  it("does not mark a pause as a segment", () => {
    const html = renderToStaticMarkup(
      wrap(
        <Transcript
          result={result}
          timecodes={false}
          currentMs={null}
          onSeek={() => undefined}
          onOpen={() => undefined}
        />,
      ),
    );
    expect(html).not.toContain('class="segment active"');
  });

  it("keeps the plain text when the engine gave no segments", () => {
    const html = renderToStaticMarkup(
      wrap(
        <Transcript
          result={{ ...result, segments: [] }}
          timecodes
          currentMs={null}
          onSeek={() => undefined}
          onOpen={() => undefined}
        />,
      ),
    );
    expect(html).toContain(translate("en", "files.noSegments"));
    expect(html).toContain("привет мир");
  });

  it("drops the timecode column when the switch is off", () => {
    const html = renderToStaticMarkup(
      wrap(
        <Transcript
          result={result}
          timecodes={false}
          currentMs={null}
          onSeek={() => undefined}
          onOpen={() => undefined}
        />,
      ),
    );
    expect(html).not.toContain("segment-times");
    // The button keeps the time for a screen reader; the row itself shows none of it
    expect(html).not.toContain("→");
    expect(html).toContain("привет");
  });

  it("carries the timecodes into the copied text only when they are on", () => {
    const first = result.segments?.[0] ?? { start_ms: 0, end_ms: 0, text: "" };
    expect(renderFileResult(result, false)).toBe(result.text);
    expect(renderFileResult(result, true)).toBe(
      "[00:00.000 → 00:02.500] привет\n[00:02.500 → 00:04.000] мир",
    );
    expect(renderSegment(first, false)).toBe("привет");
    expect(renderSegment(first, true)).toBe("[00:00.000 → 00:02.500] привет");
  });
});

describe("ProgressBar", () => {
  it("claims no percentage when the share is unknown", () => {
    const html = renderToStaticMarkup(
      <ProgressBar percent={null} label="engine" />,
    );
    expect(html).toContain("indeterminate");
    expect(html).not.toContain("aria-valuenow");
    expect(html).toContain('aria-valuetext="engine"');
  });

  it("fills exactly the known share", () => {
    const html = renderToStaticMarkup(<ProgressBar percent={40} label="half" />);
    expect(html).toContain('aria-valuenow="40"');
    expect(html).toContain("width:40%");
    expect(html).not.toContain("indeterminate");
  });

  it("never shows more than a full bar", () => {
    const html = renderToStaticMarkup(<ProgressBar percent={140} label="done" />);
    expect(html).toContain('aria-valuenow="100"');
    expect(html).toContain("width:100%");
  });
});

describe("PathInput", () => {
  it("is a closed combobox over a list until the user types and focuses", () => {
    const html = renderToStaticMarkup(
      wrap(
        <PathInput
          id="files-path"
          value="/home/u/mus"
          onChange={() => undefined}
          onStart={() => undefined}
          suggest={async () => []}
        />,
      ),
    );
    expect(html).toContain('role="combobox"');
    expect(html).toContain('aria-autocomplete="list"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain('role="listbox"');
    expect(html).not.toContain("aria-activedescendant");
  });
});

describe("the Files tab", () => {
  const view = (
    <I18nContext.Provider value={i18n}>
      <FilesProvider onError={() => undefined}>
        <Files
          config={null}
          status={null}
          reloadConfig={async () => undefined}
          refreshStatus={async () => undefined}
          onError={() => undefined}
        />
      </FilesProvider>
    </I18nContext.Provider>
  );

  it("hosts the transcript search that used to be its own tab", () => {
    const html = renderToStaticMarkup(view);
    expect(html).toContain(translate("en", "search.title"));
    expect(html).toContain(translate("en", "search.query"));
    expect(html).toContain(translate("en", "files.queue"));
    expect(html).toContain(translate("en", "files.timecodes"));
  });

  it("offers the path suggestions and the queue on the same page", () => {
    const html = renderToStaticMarkup(view);
    expect(html).toContain('role="combobox"');
    expect(html).toContain(translate("en", "files.suggestionHint"));
    expect(html).toContain(translate("en", "files.queueEmpty"));
  });
});

describe("a job of the queue", () => {
  it("says the window interrupted it instead of pretending it finished", () => {
    const job = {
      ...newJob("job-interrupted", "/media/talk.wav"),
      status: "interrupted" as const,
      started_at: 1_000,
    };
    const html = renderToStaticMarkup(
      <I18nContext.Provider value={i18n}>
        <FilesProvider onError={() => undefined}>
          <JobCell
            job={job}
            now={5_000}
            timecodes
            onOpen={() => undefined}
          />
        </FilesProvider>
      </I18nContext.Provider>,
    );
    expect(html).toContain(translate("en", "files.statusInterrupted"));
    expect(html).toContain(translate("en", "files.interruptedHint"));
    expect(html).toContain(translate("en", "files.retry"));
  });

  it("shows the elapsed time of a running job but no invented percentage", () => {
    const job = {
      ...newJob("job-running", "/media/talk.wav"),
      status: "running" as const,
      started_at: 1_000,
      progress: { phase: "engine" as const, percent: null, elapsed_ms: 0 },
    };
    const html = renderToStaticMarkup(
      <I18nContext.Provider value={i18n}>
        <FilesProvider onError={() => undefined}>
          <JobCell
            job={job}
            now={66_000}
            timecodes
            onOpen={() => undefined}
          />
        </FilesProvider>
      </I18nContext.Provider>,
    );
    expect(html).toContain(translate("en", "files.phaseEngine"));
    expect(html).toContain("01:05");
    expect(html).toContain(translate("en", "files.progressUnknown"));
    expect(html).not.toContain("aria-valuenow");
  });
});

describe("the navigation", () => {
  it("has no separate Search tab and keeps Files", () => {
    const html = renderToStaticMarkup(<App />);
    expect(html).toContain(">Files<");
    expect(html).not.toContain(">Search<");
    expect(html).toContain(">History<");
  });
});
