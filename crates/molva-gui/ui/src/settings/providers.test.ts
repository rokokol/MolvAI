// SPDX-License-Identifier: MIT
// The provider → engine → model chain: no Whisper file is offered for GigaAM and no GigaAM
// file for Whisper

import { describe, expect, it } from "vitest";

import type { SttModelOption } from "../types";
import {
  engineOfModel,
  humanSize,
  localEngines,
  modelFitsEngine,
  modelsForEngine,
  pickModel,
  providerOfEngine,
  REMOTE_ENGINE,
} from "./providers";

const catalog: SttModelOption[] = [
  { name: "small", engine: "whisper-cpp", size_bytes: 487601967, installed: true },
  { name: "large-v3-turbo", engine: "whisper-cpp", size_bytes: 1624555275, installed: false },
  {
    name: "gigaam-v3-e2e-rnnt-q4_k_m",
    engine: "transcribe-cpp",
    language: "ru",
    size_bytes: 183948704,
    installed: false,
  },
];

describe("providerOfEngine", () => {
  it("reads the provider from the engine, not from a list of its own", () => {
    expect(providerOfEngine("whisper-cpp")).toBe("local");
    expect(providerOfEngine("transcribe-cpp")).toBe("local");
    expect(providerOfEngine(REMOTE_ENGINE)).toBe("remote");
  });
});

describe("localEngines", () => {
  it("lists each engine once and keeps the catalog order", () => {
    expect(localEngines(catalog)).toEqual(["whisper-cpp", "transcribe-cpp"]);
    expect(localEngines([...catalog, ...catalog])).toEqual(["whisper-cpp", "transcribe-cpp"]);
  });

  it("never offers the remote engine as a local one", () => {
    const withRemote = [
      ...catalog,
      { name: "whisper-large-v3", engine: REMOTE_ENGINE, size_bytes: 1, installed: false },
    ];
    expect(localEngines(withRemote)).not.toContain(REMOTE_ENGINE);
  });
});

describe("modelsForEngine", () => {
  it("keeps only the models of the engine", () => {
    expect(modelsForEngine(catalog, "whisper-cpp").map((m) => m.name)).toEqual([
      "small",
      "large-v3-turbo",
    ]);
    expect(modelsForEngine(catalog, "transcribe-cpp").map((m) => m.name)).toEqual([
      "gigaam-v3-e2e-rnnt-q4_k_m",
    ]);
  });

  it("offers nothing for an engine that has no model", () => {
    expect(modelsForEngine(catalog, "vosk")).toEqual([]);
  });
});

describe("modelFitsEngine", () => {
  it("rejects the model of the other engine", () => {
    expect(modelFitsEngine(catalog, "small", "whisper-cpp")).toBe(true);
    expect(modelFitsEngine(catalog, "small", "transcribe-cpp")).toBe(false);
    expect(modelFitsEngine(catalog, "gigaam-v3-e2e-rnnt-q4_k_m", "transcribe-cpp")).toBe(true);
    expect(modelFitsEngine(catalog, "unknown", "whisper-cpp")).toBe(false);
  });
});

describe("engineOfModel", () => {
  it("finds the engine of a catalog name and gives nothing for an unknown one", () => {
    expect(engineOfModel(catalog, "gigaam-v3-e2e-rnnt-q4_k_m")).toBe("transcribe-cpp");
    expect(engineOfModel(catalog, "large-v3")).toBeUndefined();
  });
});

describe("pickModel", () => {
  it("keeps a model that the engine can load", () => {
    expect(pickModel(catalog, "whisper-cpp", "large-v3-turbo")).toBe("large-v3-turbo");
  });

  it("replaces a model of the other engine with an installed one first", () => {
    expect(pickModel(catalog, "whisper-cpp", "gigaam-v3-e2e-rnnt-q4_k_m")).toBe("small");
    expect(pickModel(catalog, "transcribe-cpp", "small")).toBe("gigaam-v3-e2e-rnnt-q4_k_m");
  });

  it("falls back to the first model when nothing is installed", () => {
    const nothingInstalled = catalog.map((model) => ({ ...model, installed: false }));
    expect(pickModel(nothingInstalled, "whisper-cpp", "gigaam-v3-e2e-rnnt-q4_k_m")).toBe("small");
  });

  it("gives an empty name for an engine without models", () => {
    expect(pickModel(catalog, "vosk", "small")).toBe("");
  });
});

describe("humanSize", () => {
  it("shows mebibytes and gibibytes", () => {
    expect(humanSize(183948704)).toBe("175 MiB");
    expect(humanSize(1624555275)).toBe("1.5 GiB");
    expect(humanSize(0)).toBe("0 MiB");
  });
});
