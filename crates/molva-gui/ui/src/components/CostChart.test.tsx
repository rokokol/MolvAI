// SPDX-License-Identifier: MIT

import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";

import CostChart from "./CostChart";

const day = (day: string, cost_usd: number | null, incomplete = false) => ({
  day, entries: 1,
  stt_cost: { cost_usd, incomplete },
  llm_cost: { cost_usd: 0, incomplete: false },
});

test("daily costs include zero, mark partial amounts, and leave a gap for unknown prices", () => {
  const html = renderToStaticMarkup(<CostChart kind="stt_cost" series={[
    day("2026-09-01", 0),
    day("2026-09-02", 0.000029, true),
    day("2026-09-03", null, true),
    day("2026-09-04", 0.000018),
  ]} />);

  expect(html).toContain("<svg");
  expect(html.match(/class="cost-line" d="([^"]+)"/)?.[1]?.match(/[ML]/g)).toEqual(["M", "L", "M"]);
  expect(html).toContain("<td>$0</td>");
  expect(html).toContain("$0.000029");
  expect(html).toContain("not reported");
  expect(html).toContain("partial");
});

test("unknown recognition prices do not interrupt the LLM chart", () => {
  const series = [
    { day: "2026-09-01", entries: 1, stt_cost: { cost_usd: 0.2, incomplete: false }, llm_cost: { cost_usd: 0.03, incomplete: false } },
    { day: "2026-09-02", entries: 1, stt_cost: { cost_usd: null, incomplete: true }, llm_cost: { cost_usd: 0.04, incomplete: false } },
    { day: "2026-09-03", entries: 1, stt_cost: { cost_usd: 0.1, incomplete: false }, llm_cost: { cost_usd: 0.02, incomplete: false } },
  ];
  const stt = renderToStaticMarkup(<CostChart series={series} kind="stt_cost" />);
  const llm = renderToStaticMarkup(<CostChart series={series} kind="llm_cost" />);
  expect(stt).toContain("not reported");
  expect(llm).not.toContain("not reported");
  expect(llm).toContain("<td>$0.04</td>");
  expect(stt).not.toContain("<td>$0.04</td>");
});
