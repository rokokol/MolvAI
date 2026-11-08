// SPDX-License-Identifier: MIT
// The result of the two test buttons of the settings page: the model and the recognition
//
// A test works from the draft of the form and writes nothing. The page shows three states in the
// place of the button: a pending line, the failure with its next step, and the report

import { useI18n } from "../i18n";
import type { CommandError, LlmProbeReport, SttProbeReport } from "../types";

/** A test that has not run yet */
export interface ProbeState<T> {
  pending: boolean;
  report: T | null;
  error: CommandError | null;
}

/** The state before the first press, and the state to return to when the draft changes */
export function idleProbe<T>(): ProbeState<T> {
  return { pending: false, report: null, error: null };
}

/** A failure of either test: what happened and what to do next */
function ProbeFailure({ state, title }: { state: ProbeState<unknown>; title: string }) {
  if (!state.error) {
    return null;
  }
  return (
    <div className="notice error" role="alert">
      <p className="small">
        <strong>{title}</strong>
        {`: ${state.error.message}`}
      </p>
      {state.error.hint && <p className="small">{state.error.hint}</p>}
    </div>
  );
}

/** What the model test found: the provider, the model, the time and the answer */
export function LlmProbeResult({ state }: { state: ProbeState<LlmProbeReport> }) {
  const { t } = useI18n();
  if (state.error) {
    return <ProbeFailure state={state} title={t("settings.llm.testFailed")} />;
  }
  const report = state.report;
  if (!report) {
    return null;
  }
  return (
    <div className="notice ok" role="status">
      <p className="small">
        {t("settings.llm.testOk", {
          provider: report.provider,
          model: report.model,
          latency: report.latency_ms,
        })}
      </p>
      <p className="small muted">
        {t("settings.llm.testAnswer", { answer: report.answer })}
        {" · "}
        {t("settings.llm.testRetries", { attempts: report.attempts })}
      </p>
      {!report.is_local && <p className="small muted">{t("settings.llm.testCloudNote")}</p>}
    </div>
  );
}

/** What the recognition test found: one line per check, each with its own reason */
export function SttProbeResult({ state }: { state: ProbeState<SttProbeReport> }) {
  const { t } = useI18n();
  if (state.error) {
    return <ProbeFailure state={state} title={t("settings.sttTest.testFailed")} />;
  }
  const report = state.report;
  if (!report) {
    return null;
  }
  return (
    <div className={`notice ${report.ok ? "ok" : "warning"}`} role="status">
      <p className="small">
        <strong>
          {report.ok ? t("settings.sttTest.ok") : t("settings.sttTest.failed")}
        </strong>
      </p>
      <ul>
        {report.checks.map((check) => (
          <li key={check.id}>
            {check.ok ? "✓" : "✗"} {t(`settings.sttTest.check.${check.id}`)}
            {check.detail !== "" && <span className="mono small">{` — ${check.detail}`}</span>}
          </li>
        ))}
      </ul>
    </div>
  );
}
