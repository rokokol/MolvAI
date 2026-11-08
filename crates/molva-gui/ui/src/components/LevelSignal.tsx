// SPDX-License-Identifier: MIT
// The live signal of the dashboard: one pixel column per level event, drawn as DOM cells
//
// The columns are the levels the daemon already sends. There is no canvas and no animation
// frame: a level event appends one column, and React paints only that change. That keeps the
// picture live without a WebKit render loop

import { useEffect, useState } from "react";

import { useI18n } from "../i18n";
import {
  signalPeak,
  signalPercent,
  signalValue,
} from "./level-signal";

/** How many columns the window shows */
const COLUMNS = 48;

export default function LevelSignal({ level, active }: { level: number; active: boolean }) {
  const { t } = useI18n();
  const [history, setHistory] = useState<number[]>([]);

  useEffect(() => {
    if (!active) {
      setHistory([]);
      return;
    }
    setHistory((previous) => [...previous, signalValue(level)].slice(-COLUMNS));
  }, [active, level]);

  const percent = signalPercent(level);
  const peak = Math.round(Math.max(signalValue(level), signalPeak(history)) * 100);
  const values = [...Array<number>(COLUMNS - history.length).fill(0), ...history];

  return (
    <div className="signal-wrap">
      <div
        className="signal-bars"
        role="img"
        aria-label={
          active
            ? t("dashboard.signalLabel", { level: percent })
            : t("dashboard.signalIdle")
        }
      >
        {values.map((value, index) => (
          <span
            key={index}
            className={value > 0 ? "signal-column lit" : "signal-column"}
            style={{ height: `${Math.round(value * 100)}%` }}
          />
        ))}
      </div>
      <div className="signal-foot">
        <p className="signal-readout" aria-hidden="true">
          {active
            ? t("dashboard.signalReadout", { level: percent, peak })
            : t("dashboard.signalIdle")}
        </p>
        <p className="small muted signal-hint">{t("dashboard.signalHint")}</p>
      </div>
    </div>
  );
}
