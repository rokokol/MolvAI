// SPDX-License-Identifier: MIT

import { formatCost, formatDayTick } from "../format";
import { useI18n } from "../i18n";
import type { DayPoint } from "../types";

const TITLES = {
  stt_cost: "stats.sttCostChart",
  llm_cost: "stats.llmCostChart",
} as const;

type CostKind = keyof typeof TITLES;
type CostDay = Pick<DayPoint, "day" | "entries" | CostKind>;

const WIDTH = 760;
const HEIGHT = 220;
const LEFT = 108;
const TOP = 14;
const PLOT_WIDTH = WIDTH - LEFT - 26;
const PLOT_HEIGHT = HEIGHT - TOP - 26;

export default function CostChart({ series, kind }: { series: CostDay[]; kind: CostKind }) {
  const { t } = useI18n();
  const title = t(TITLES[kind]);
  if (!series.some((point) => point.entries > 0)) {
    return (
      <section className="card">
        <h2>{title}</h2>
        <p className="empty">{t("stats.chartEmpty")}</p>
      </section>
    );
  }

  const costs = series.map(({ day, [kind]: cost }) => ({ day, ...cost }));
  const max = Math.max(0, ...costs.map((point) => point.cost_usd ?? 0));
  const slot = PLOT_WIDTH / series.length;
  const yFor = (value: number) => PLOT_HEIGHT - (max > 0 ? value / max : 0) * PLOT_HEIGHT;
  const tickEvery = Math.ceil(series.length / 8);
  const points = costs.map((point, index) => ({
    ...point,
    x: (index + 0.5) * slot,
    y: point.cost_usd === null ? null : yFor(point.cost_usd),
    label: point.cost_usd === null ? t("stats.costUnknown") :
      `${formatCost(point.cost_usd)}${point.incomplete ? ` (${t("stats.costDayPartial")})` : ""}`,
  }));
  const path = points.map(({ x, y }, index) => {
    if (y === null) return "";
    const previous = points[index - 1];
    return `${previous && previous.y !== null ? "L" : "M"}${x} ${y}`;
  }).join(" ").trim();

  return (
    <section className="card">
      <h2>{title}</h2>
      <svg className="chart" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label={title}>
        <g transform={`translate(${LEFT} ${TOP})`}>
          {(max > 0 ? [0, max / 2, max] : [0]).map((value) => (
            <g key={value}>
              <line className="grid-line" x1={0} x2={PLOT_WIDTH} y1={yFor(value)} y2={yFor(value)} />
              <text x={-8} y={yFor(value) + 6} textAnchor="end">{formatCost(value)}</text>
            </g>
          ))}
          {path && <path className="cost-line" d={path} />}
          {points.map(({ day, x, y, incomplete }, index) => (
            <g key={day}>
              {y !== null && (
                <circle className={`cost-dot${incomplete ? " partial" : ""}`} cx={x} cy={y} r={4} />
              )}
              {index % tickEvery === 0 && (
                <text x={x} y={PLOT_HEIGHT + 18} textAnchor="middle">{formatDayTick(day)}</text>
              )}
            </g>
          ))}
          {points.map(({ day, x, label }) => (
            <g className="cost-hover" key={day}>
              <line className="cost-guide" x1={x} x2={x} y1={0} y2={PLOT_HEIGHT} />
              <rect className="cost-target" x={x - slot / 2} width={slot} height={PLOT_HEIGHT + 4}>
                <title>{`${day}: ${label}`}</title>
              </rect>
            </g>
          ))}
        </g>
      </svg>
      {costs.some((point) => point.incomplete) && <p className="small muted">{t("stats.costChartNote")}</p>}
      <details className="small">
        <summary>{t("stats.costValues")}</summary>
        <div className="table-scroll">
          <table>
            <thead><tr><th scope="col">{t("stats.costDay")}</th><th scope="col">USD</th></tr></thead>
            <tbody>
              {points.map(({ day, label }) => <tr key={day}><th scope="row">{day}</th><td>{label}</td></tr>)}
            </tbody>
          </table>
        </div>
      </details>
    </section>
  );
}
