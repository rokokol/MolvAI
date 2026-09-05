// SPDX-License-Identifier: MIT
// График слов по дням и среднего темпа. Рисуется руками в SVG: сторонние
// библиотеки графиков сюда не тянем — лишние лицензии и вес ради двух фигур.

import { axisTicks, chartGeometry, formatDayTick } from "../format";
import { useI18n } from "../i18n";
import type { DayPoint } from "../types";

const WIDTH = 760;
const HEIGHT = 220;
const PADDING = { top: 12, right: 46, bottom: 26, left: 46 };

export default function WordsChart({ series }: { series: DayPoint[] }) {
  const { t } = useI18n();
  if (series.length === 0) {
    return <p className="muted">{t("stats.chartEmpty")}</p>;
  }

  const plotWidth = WIDTH - PADDING.left - PADDING.right;
  const plotHeight = HEIGHT - PADDING.top - PADDING.bottom;
  const geometry = chartGeometry(series, plotWidth, plotHeight);
  // Подписей по оси дней не больше десяти: иначе они наезжают друг на друга.
  const tickEvery = Math.max(1, Math.ceil(series.length / 10));
  const wordTicks = axisTicks(geometry.maxWords);
  const wpmTicks = axisTicks(geometry.maxWpm);

  return (
    <>
      <svg
        className="chart"
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={t("stats.chart")}
        preserveAspectRatio="xMidYMid meet"
      >
        <g transform={`translate(${PADDING.left} ${PADDING.top})`}>
          {wordTicks.map((value) => {
            const y = plotHeight - (value / geometry.maxWords) * plotHeight;
            return (
              <g key={`grid-${value}`}>
                <line className="grid-line" x1={0} x2={plotWidth} y1={y} y2={y} />
                <text x={-8} y={y + 4} textAnchor="end">
                  {value}
                </text>
              </g>
            );
          })}
          {wpmTicks.map((value) => {
            const y = plotHeight - (value / geometry.maxWpm) * plotHeight;
            return (
              <text key={`wpm-${value}`} x={plotWidth + 8} y={y + 4} textAnchor="start">
                {value}
              </text>
            );
          })}
          {geometry.points.map((point) => (
            <rect
              key={point.day}
              className="bar"
              x={point.x}
              y={point.y}
              width={point.width}
              height={Math.max(0, point.height)}
            >
              <title>
                {point.day}: {point.words} {t("common.words")}
              </title>
            </rect>
          ))}
          {geometry.wpmPath && <path className="wpm-line" d={geometry.wpmPath} />}
          {geometry.points.map((point) =>
            point.wpmY === null ? null : (
              <circle
                key={`dot-${point.day}`}
                className="wpm-dot"
                cx={point.x + point.width / 2}
                cy={point.wpmY}
                r={2.5}
              >
                <title>
                  {point.day}: {point.wpm?.toFixed(1)} {t("common.wpm")}
                </title>
              </circle>
            ),
          )}
          {geometry.points.map((point, index) =>
            index % tickEvery === 0 ? (
              <text
                key={`tick-${point.day}`}
                x={point.x + point.width / 2}
                y={plotHeight + 16}
                textAnchor="middle"
              >
                {formatDayTick(point.day)}
              </text>
            ) : null,
          )}
        </g>
      </svg>
      <div className="legend">
        <span>
          <i style={{ background: "var(--chart-bar)" }} aria-hidden="true" />
          {t("stats.legendWords")}
        </span>
        <span>
          <i style={{ background: "var(--chart-line)" }} aria-hidden="true" />
          {t("stats.legendWpm")}
        </span>
      </div>
    </>
  );
}
