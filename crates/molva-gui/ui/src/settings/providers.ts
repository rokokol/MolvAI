// SPDX-License-Identifier: MIT
// Recognition choice: a provider, then an engine of that provider, then a model of that engine
//
// The models and their engines come from `models.toml` through the `stt_models` command. This
// module only groups them, so the page can never offer a Whisper file for GigaAM

import type { SttModelOption } from "../types";

/** The engine name that means "recognize through a remote API" */
export const REMOTE_ENGINE = "remote-openai";

/** The key sources the remote recognizer accepts: it keeps no key in clear text in its settings */
export const STT_KEY_SOURCES = ["keyring", "env", "none"] as const;

/** A provider whose API serves transcription, as the model choice offers it */
export interface TranscribingProvider {
  id: string;
  base_url: string;
  api_key_env: string;
  /** The model this provider transcribes with, empty when the person names it by hand */
  stt_model: string | null;
}

/** The providers the recognizer offers: `stt_model` is `null` where the API transcribes nothing */
export function sttProviders<T extends TranscribingProvider>(providers: T[]): T[] {
  return providers.filter((item) => item.stt_model !== null);
}

/** Which preset an address belongs to; an address of the person's own means "set by hand" */
export function remoteSttProvider(
  providers: TranscribingProvider[],
  baseUrl: string,
): string {
  const wanted = baseUrl.trim().replace(/\/+$/, "");
  const preset = sttProviders(providers).find(
    (item) => item.base_url.trim().replace(/\/+$/, "") === wanted,
  );
  return preset?.id ?? "custom";
}

export type SttProvider = "local" | "remote";

/** A provider comes from the engine alone: there is one remote engine and the rest are local */
export function providerOfEngine(engine: string): SttProvider {
  return engine === REMOTE_ENGINE ? "remote" : "local";
}

/** The engines that run on this machine, in catalog order and without repeats */
export function localEngines(models: SttModelOption[]): string[] {
  const engines: string[] = [];
  for (const model of models) {
    if (model.engine !== REMOTE_ENGINE && !engines.includes(model.engine)) {
      engines.push(model.engine);
    }
  }
  return engines;
}

/** The models that one engine can load */
export function modelsForEngine(models: SttModelOption[], engine: string): SttModelOption[] {
  return models.filter((model) => model.engine === engine);
}

/** Whether the model belongs to the engine: the pair is what the daemon builds */
export function modelFitsEngine(
  models: SttModelOption[],
  model: string,
  engine: string,
): boolean {
  return models.some((item) => item.name === model && item.engine === engine);
}

/** The engine of a model, or `undefined` for a name that the catalog does not know */
export function engineOfModel(models: SttModelOption[], model: string): string | undefined {
  return models.find((item) => item.name === model)?.engine;
}

/**
 * The engine the page works with, from the settings and the catalog
 *
 * The catalog names the engine of a local model, but a cloud engine has no model in the catalog:
 * reading the engine from the model alone turned a chosen cloud engine back into the local one, so
 * the picker looked like it did nothing
 */
export function selectedEngine(
  models: SttModelOption[],
  configured: string,
  model: string,
): string {
  if (configured === REMOTE_ENGINE) {
    return REMOTE_ENGINE;
  }
  return engineOfModel(models, model) ?? configured;
}

/**
 * The model to keep after an engine change: the current one when it fits, else the first
 * installed one, else the first of the engine. Empty only when the engine has no model
 */
export function pickModel(
  models: SttModelOption[],
  engine: string,
  current: string,
): string {
  const candidates = modelsForEngine(models, engine);
  if (candidates.some((model) => model.name === current)) {
    return current;
  }
  return (candidates.find((model) => model.installed) ?? candidates[0])?.name ?? "";
}

/** A size for a hint beside a model: the weights are the reason a download takes minutes */
export function humanSize(bytes: number): string {
  if (bytes <= 0) {
    return "0 MiB";
  }
  const mib = bytes / (1024 * 1024);
  if (mib >= 1024) {
    return `${(mib / 1024).toFixed(1)} GiB`;
  }
  return `${Math.round(mib)} MiB`;
}
