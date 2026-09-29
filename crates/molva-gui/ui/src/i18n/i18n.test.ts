// SPDX-License-Identifier: MIT
// Выбор языка интерфейса: явная настройка важнее системы, а незнакомая система — английский.

import { describe, expect, it } from "vitest";

import { resolveLang } from "./index";

describe("resolveLang", () => {
  it("берёт язык из настройки, если он задан явно", () => {
    expect(resolveLang("ru", "en-US")).toBe("ru");
    expect(resolveLang("en", "ru-RU")).toBe("en");
  });

  it("при auto следует языку системы", () => {
    expect(resolveLang("auto", "ru-RU")).toBe("ru");
    expect(resolveLang("auto", "ru")).toBe("ru");
    expect(resolveLang("auto", "en-GB")).toBe("en");
  });

  it("незнакомый или пустой язык системы даёт английский", () => {
    expect(resolveLang("auto", "de-DE")).toBe("en");
    expect(resolveLang("auto", "")).toBe("en");
    expect(resolveLang(undefined, "C")).toBe("en");
  });

  it("мусор в настройке ведёт себя как auto", () => {
    expect(resolveLang("kz", "ru-RU")).toBe("ru");
    expect(resolveLang("kz", "fr-FR")).toBe("en");
  });
});
